//! The external tools the pipeline shells out to, and what happens without them.
//!
//! # There is no `cargo-component` here, deliberately
//!
//! The issue that specified this pipeline named `cargo component build`. That
//! is not needed: the `wasm32-wasip2` target emits a component directly through
//! rustc's own `wasm-component-ld`, and `rust-toolchain.toml` already pins the
//! target. Detecting and version-checking a tool the pipeline never invokes
//! would be work in service of nothing.
//!
//! # There is no `wasm-opt` either, and that one is not a choice
//!
//! Binaryen cannot read a component at all. Measured against v120, current at
//! the time of writing:
//!
//! ```text
//! [parse exception: this looks like a wasm component, which Binaryen does not
//! support yet (see https://github.com/WebAssembly/binaryen/issues/6728) (at 0:8)]
//! ```
//!
//! Optimising the core module *inside* the component is possible — unbundling
//! one and running `-Oz` on it measured 45,174 to 36,974 bytes — but
//! `wasm-tools component unbundle` converts embedded modules into imported
//! ones and offers no inverse. Reassembly would be homemade, and its failure
//! mode is a component that is subtly wrong rather than obviously broken. Not
//! worth 8 KB.
//!
//! What remains is `wasm-tools strip`, which removes custom sections: measured
//! at 58,618 to 52,988 bytes, 9.6%, on the reference template. The Rust release
//! profile has already stripped core names by then, so this costs no
//! debuggability that was not already gone.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::error::CliError;

/// The program name probed on `PATH`.
const WASM_TOOLS: &str = "wasm-tools";

/// How to install `wasm-tools`. An exact command, because "not found" without
/// one leaves the reader to guess at a crate name.
pub const WASM_TOOLS_INSTALL: &str = "cargo install wasm-tools";

/// How to add the compile target, for the same reason.
pub const TARGET_INSTALL: &str = "rustup target add wasm32-wasip2";

/// The compile target. Not configurable: a component is what the runtime
/// accepts, and this is the target that produces one.
pub const TARGET: &str = "wasm32-wasip2";

/// External tools, probed once.
///
/// Probing is done at construction rather than per use. Three `--version`
/// calls where one would do is harmless but silly, and a tool that appeared or
/// vanished midway through a single build would be a worse problem than the
/// one caching avoids.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Toolchain {
    wasm_tools: Option<OsString>,
}

impl Toolchain {
    /// Probe the environment.
    #[must_use]
    pub fn detect() -> Self {
        Self {
            wasm_tools: probe(WASM_TOOLS),
        }
    }

    /// Construct with an explicit program, for tests.
    ///
    /// Passing a name that is not on `PATH` is how the absent-tool path gets
    /// exercised without depending on the machine the tests run on.
    #[must_use]
    pub fn with_wasm_tools(program: Option<OsString>) -> Self {
        Self {
            wasm_tools: program,
        }
    }

    /// Whether the optional optimiser is available.
    #[must_use]
    pub fn has_wasm_tools(&self) -> bool {
        self.wasm_tools.is_some()
    }

    /// The warning to print when `wasm-tools` is absent.
    ///
    /// A warning rather than a failure: the tool is worth 9.6%, and refusing to
    /// build without it would be disproportionate to what it buys. The message
    /// names the saving so the reader can judge whether to bother, and the
    /// command so acting on it takes no searching.
    #[must_use]
    pub fn wasm_tools_warning() -> String {
        format!(
            "{WASM_TOOLS} not found — building without stripping custom sections \
             (about 9% larger). Install it with `{WASM_TOOLS_INSTALL}`."
        )
    }

    /// Strip custom sections from `input`, writing `output`.
    ///
    /// Returns `Ok(false)` when the tool is absent and nothing was done, so the
    /// caller can report the skip rather than silently producing a larger
    /// artifact than it intended.
    pub fn strip(&self, input: &Path, output: &Path) -> Result<bool, CliError> {
        let Some(program) = self.wasm_tools.as_ref() else {
            return Ok(false);
        };

        let out = Command::new(program)
            .arg("strip")
            .arg(input)
            .arg("-o")
            .arg(output)
            .output()
            .map_err(|e| CliError::ToolchainMissing {
                tool: WASM_TOOLS,
                install: WASM_TOOLS_INSTALL,
                cause: e.to_string(),
            })?;

        if !out.status.success() {
            return Err(CliError::BuildFailed {
                step: "wasm-tools strip",
                // Trimmed because the caller prints this inline. `wasm-tools`
                // writes its diagnostics to stderr; stdout here is the artifact
                // path and says nothing about the failure.
                detail: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(true)
    }
}

/// Whether `program` exists and runs.
///
/// `--version` rather than a `PATH` scan: a name on `PATH` that cannot execute
/// — wrong architecture, missing loader, a directory with the right name — is
/// indistinguishable from an absent one until it is run, and the failure would
/// otherwise surface in the middle of a build.
fn probe(program: &str) -> Option<OsString> {
    Command::new(program)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| OsString::from(program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_tool_is_reported_as_absent_rather_than_probed_again() {
        let tc = Toolchain::with_wasm_tools(None);
        assert!(!tc.has_wasm_tools());
    }

    /// The absent path must skip, not fail. A tool worth 9% should not be able
    /// to stop a build.
    #[test]
    fn stripping_without_the_tool_skips_and_says_it_skipped() {
        let tc = Toolchain::with_wasm_tools(None);
        let did = tc
            .strip(Path::new("in.wasm"), Path::new("out.wasm"))
            .expect("an absent optimiser must not fail the build");
        assert!(
            !did,
            "the skip must be reported, or the caller cannot tell a stripped \
             artifact from an unstripped one"
        );
    }

    /// The message has to be actionable on its own — the reader is looking at a
    /// terminal, not at this file.
    #[test]
    fn the_missing_tool_warning_names_the_command_and_the_cost() {
        let w = Toolchain::wasm_tools_warning();
        assert!(w.contains(WASM_TOOLS_INSTALL), "no install command in: {w}");
        assert!(
            w.contains('%'),
            "the warning should quantify what is lost, got: {w}"
        );
    }

    /// A name that is not on `PATH` must probe as absent rather than panicking
    /// or being treated as present.
    #[test]
    fn probing_a_nonexistent_program_yields_none() {
        assert!(probe("clwd-definitely-not-a-real-program-xyz").is_none());
    }

    /// The remediation command must be in the rendered message, not merely
    /// carried in a field nobody prints. "Not found" without the command is
    /// exactly the unactionable failure this type exists to avoid.
    #[test]
    fn an_unrunnable_tool_prints_the_exact_install_command() {
        let tc = Toolchain::with_wasm_tools(Some(OsString::from(
            "clwd-definitely-not-a-real-program-xyz",
        )));
        let err = tc
            .strip(Path::new("in.wasm"), Path::new("out.wasm"))
            .expect_err("must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains(WASM_TOOLS_INSTALL),
            "the failure must name the fix, got: {rendered}"
        );
    }

    /// A configured tool that cannot execute must surface as a typed error, not
    /// a panic — the whole point of probing with `--version` first.
    #[test]
    fn a_configured_but_unrunnable_tool_is_a_typed_error() {
        let tc = Toolchain::with_wasm_tools(Some(OsString::from(
            "clwd-definitely-not-a-real-program-xyz",
        )));
        let err = tc
            .strip(Path::new("in.wasm"), Path::new("out.wasm"))
            .expect_err("an unrunnable tool must be an error, not a skip");
        assert_eq!(err.code(), "cli.toolchain_missing");
    }
}
