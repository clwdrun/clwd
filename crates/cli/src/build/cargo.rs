//! Driving `cargo`, and reading the project's release profile.
//!
//! # The artifact path is read, not guessed
//!
//! `target/wasm32-wasip2/release/<name>.wasm` looks predictable until the
//! package is called `my-app` and the file is `my_app.wasm`, or the project
//! sets `[lib] name`, or a workspace redirects `target/`. Rather than
//! reimplementing cargo's naming rules, this asks cargo: `--message-format=json`
//! emits a `compiler-artifact` record per crate, carrying the real paths.
//!
//! The stream is read line by line as cargo produces it, so progress can be
//! reported while the build runs. Buffering it to parse afterwards would make a
//! reporter that only fires once the work it is reporting on has finished.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::Progress;
use crate::error::CliError;

use super::toolchain::{TARGET, TARGET_INSTALL};

/// Settings the release profile should carry, and what each is worth.
///
/// Measured: the reference template is ~58 KB with these and several times that
/// without. This is the single largest lever on component size, which is why a
/// build says something when it is missing — see [`profile_warnings`].
const PROFILE_ADVICE: &[(&str, &str)] = &[
    (
        "opt-level",
        "opt-level = \"z\"    # optimise for size, not speed",
    ),
    (
        "lto",
        "lto = true           # let the linker drop what nothing calls",
    ),
    (
        "strip",
        "strip = true         # no symbol names in a shipped artifact",
    ),
];

/// Complain about a release profile that will produce a needlessly large
/// component.
///
/// A warning, never a failure. Templates (#29) ship the right profile, but #29
/// only controls what a template starts as: an author who copies one into a
/// fresh project, or edits `Cargo.toml` later, loses the setting with nothing
/// to tell them why their component grew. This is the check that catches the
/// case a template cannot.
///
/// Absent `[profile.release]` is reported as one line rather than three, since
/// the fix is to paste one block rather than to hunt three keys.
#[must_use]
pub fn profile_warnings(cargo_toml: &str) -> Vec<String> {
    // `Table`, not `Value`. `Value`'s `FromStr` parses one TOML *value*, so a
    // document beginning `[profile.release]` is read as an array and then fails
    // on the rest of the file with "unexpected content, expected nothing" —
    // which looks exactly like a malformed manifest and silently disabled every
    // warning below.
    let Ok(doc) = cargo_toml.parse::<toml::Table>() else {
        // A manifest cargo itself will reject. Let cargo produce that error
        // rather than guessing at one here.
        return Vec::new();
    };
    let profile = doc.get("profile").and_then(|p| p.get("release"));

    let Some(profile) = profile else {
        return vec![format!(
            "no [profile.release] in Cargo.toml — the component will be much larger \
             than it needs to be. Add:\n\n[profile.release]\n{}\n",
            PROFILE_ADVICE
                .iter()
                .map(|(_, line)| *line)
                .collect::<Vec<_>>()
                .join("\n")
        )];
    };

    PROFILE_ADVICE
        .iter()
        .filter(|(key, _)| profile.get(key).is_none())
        .map(|(key, line)| {
            format!("[profile.release] has no `{key}` — add `{line}` to shrink the component")
        })
        .collect()
}

/// Compile the project to a wasm component and return the artifact's path.
pub fn compile(project: &Path, progress: Option<&dyn Progress>) -> Result<PathBuf, CliError> {
    let mut child = Command::new("cargo")
        .current_dir(project)
        .args([
            "build",
            "--release",
            "--target",
            TARGET,
            // Diagnostics still render as text; only the metadata is JSON. A
            // plain `--message-format=json` would turn a compile error into
            // unreadable escaped JSON for the person who has to fix it.
            "--message-format=json-render-diagnostics",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| CliError::ToolchainMissing {
            tool: "cargo",
            install: "install Rust from https://rustup.rs",
            cause: e.to_string(),
        })?;

    let mut artifact = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if msg.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
                continue;
            }
            if let Some(name) = msg.pointer("/target/name").and_then(|n| n.as_str())
                && let Some(p) = progress
            {
                p.step(&format!("compiling {name}"));
            }
            // The last `.wasm` wins: dependencies are emitted before the crate
            // being built, so the final one is the artifact asked for.
            if let Some(files) = msg.get("filenames").and_then(|f| f.as_array()) {
                for f in files.iter().filter_map(|f| f.as_str()) {
                    if f.ends_with(".wasm") {
                        artifact = Some(PathBuf::from(f));
                    }
                }
            }
        }
    }

    let status = child.wait().map_err(|e| CliError::BuildFailed {
        step: "cargo build",
        detail: e.to_string(),
    })?;

    if !status.success() {
        return Err(CliError::BuildFailed {
            step: "cargo build",
            // cargo's diagnostics already went to the inherited stderr, where
            // the reader has seen them. Repeating them here would double every
            // compile error.
            detail: format!(
                "cargo exited unsuccessfully. If it reported a missing target, run `{TARGET_INSTALL}`"
            ),
        });
    }

    artifact.ok_or(CliError::BuildFailed {
        step: "cargo build",
        detail: format!(
            "the build succeeded but produced no .wasm for {TARGET}. Is the crate a \
             `crate-type = [\"cdylib\"]` library?"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_release_profile_draws_no_complaint() {
        let toml = r#"
[package]
name = "x"
[profile.release]
opt-level = "z"
lto = true
strip = true
"#;
        assert!(profile_warnings(toml).is_empty());
    }

    /// The specific missing key, not a generic grumble. An author who is told
    /// "your profile is wrong" has to work out which part.
    #[test]
    fn a_missing_setting_is_named_individually() {
        let toml = r#"
[profile.release]
opt-level = "z"
lto = true
"#;
        let w = profile_warnings(toml);
        assert_eq!(w.len(), 1, "expected exactly one warning, got {w:?}");
        assert!(w[0].contains("strip"), "got: {w:?}");
    }

    /// The case that motivates this check existing: a fresh project copied from
    /// a template, with the profile left behind.
    #[test]
    fn an_absent_release_profile_is_one_warning_carrying_the_whole_block() {
        let w = profile_warnings("[package]\nname = \"x\"\n");
        assert_eq!(w.len(), 1, "one paste-able fix, not three hunts: {w:?}");
        for (key, _) in PROFILE_ADVICE {
            assert!(w[0].contains(key), "{key} missing from: {}", w[0]);
        }
        assert!(w[0].contains("[profile.release]"));
    }

    /// Warnings only. A build with a bad profile still builds — the artifact is
    /// correct, merely larger than it should be.
    #[test]
    fn a_bad_profile_produces_warnings_rather_than_an_error() {
        let w = profile_warnings("[profile.release]\nopt-level = 0\n");
        assert!(!w.is_empty());
        // The signature returns no Result at all; this test exists so that
        // changing it to one is a visible decision rather than a quiet edit.
    }

    /// An unparseable manifest is cargo's error to report, with a line number.
    /// Guessing at one here would produce a worse message for the same problem.
    #[test]
    fn an_unparseable_manifest_defers_to_cargo_rather_than_inventing_a_warning() {
        assert!(profile_warnings("this is not [ toml").is_empty());
    }
}
