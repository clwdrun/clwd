//! `clwd build` — source tree to deployable bundle.
//!
//! The pipeline is subprocess-bound: `cargo build --target wasm32-wasip2` takes
//! tens of seconds and everything this module does around it takes single-digit
//! milliseconds. Measured in one session: a clean build 31.10s, `wasm-tools
//! strip` 0.00s, bundle assembly below timer resolution. Optimising the Rust
//! here would be measuring the wrong thing.
//!
//! # Why there is no build cache
//!
//! There was going to be one. Then a no-op rebuild measured **0.07s**, because
//! cargo already does exactly this and does it well. A content-addressed cache
//! in front of that would be machinery costing more than it saves.
//!
//! What is here instead is a digest comparison: if the component and the sealed
//! manifest are both byte-identical to what the existing bundle contains, the
//! bundle is not rewritten. That skips a write, not a build.
//!
//! **If anything is ever persisted across runs, this changes.** A cached
//! `wasm-tools strip` output is invalid across `wasm-tools` versions, exactly as
//! `clwd-runtime`'s component cache is valid keyed on the bundle digest alone
//! only because it lives in memory — see the note on `ComponentCache`. The
//! moment either goes to disk, the tool version belongs in the key.

pub mod cargo;
pub mod manifest;
pub mod process;
pub mod size;
pub mod toolchain;

use std::path::{Path, PathBuf};

use clwd_bundle::File;

pub use manifest::Source;
pub use process::{Cancel, Tail};
pub use size::{BundleSize, ComponentSize, Verdict};
pub use toolchain::Toolchain;

use crate::Progress;
use crate::error::CliError;

/// The bundle extension. One artifact, one suffix.
const BUNDLE_EXT: &str = "clwd";

/// What to build, and how.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Options {
    /// The project directory: the one with `Cargo.toml` in it.
    pub project: PathBuf,
    /// Where to write the bundle. Defaults beside the component.
    pub out: Option<PathBuf>,
    /// Strip custom sections. `--no-optimize` turns this off.
    pub optimize: bool,
    /// Keep cargo's stderr here instead of inheriting it.
    pub output: Option<Tail>,
    /// Lets another thread stop the build.
    pub cancel: Option<Cancel>,
}

impl Options {
    /// Build `project` with the defaults.
    #[must_use]
    pub fn new(project: PathBuf) -> Self {
        Self {
            project,
            out: None,
            optimize: true,
            output: None,
            cancel: None,
        }
    }
}

/// What a completed build produced.
///
/// `#[must_use]`: every field here is either something the caller prints or
/// something it checks, so dropping the whole thing silently is a bug rather
/// than a style question.
#[must_use]
#[derive(Debug, serde::Serialize)]
#[non_exhaustive]
pub struct Built {
    /// Where the bundle was written.
    pub bundle: PathBuf,
    /// Its content address.
    pub digest: String,
    /// The component, which is what drives cold start.
    pub component_size: ComponentSize,
    /// The bundle, which is what drives transfer.
    pub bundle_size: BundleSize,
    /// Whether custom sections were stripped.
    pub stripped: bool,
    /// Whether the bundle was already current and left alone.
    pub unchanged: bool,
    /// The capability set sealed into the manifest, rendered for display.
    ///
    /// Always present, including when everything is empty. Deny-by-default is
    /// only visible if a grant of nothing is printed as loudly as a grant of
    /// something; an author who sees nothing learns nothing.
    pub capabilities: Vec<String>,
    /// Anything the build wants said but not enough to fail over.
    pub warnings: Vec<String>,
}

/// Render the sealed capability set for display.
fn describe(caps: &clwd_bundle::Capabilities) -> Vec<String> {
    let list = |name: &str, v: &[String]| {
        if v.is_empty() {
            format!("{name}: none")
        } else {
            format!("{name}: {}", v.join(", "))
        }
    };
    vec![
        list("egress", &caps.egress),
        list("secrets", &caps.secrets),
        list("storage", &caps.storage_namespaces),
        format!(
            "limits: {} ms cpu, {} MB memory",
            caps.limits.cpu_ms, caps.limits.memory_mb
        ),
    ]
}

/// Run the pipeline.
pub fn run(options: &Options, progress: Option<&dyn Progress>) -> Result<Built, CliError> {
    let mut warnings = Vec::new();

    // Checked before anything is spawned. Without this, a missing directory
    // surfaces as `cargo` failing to start, which `compile` reports as
    // `cli.toolchain_missing` — telling the caller to install a tool they
    // already have, about a path they mistyped.
    let cargo_toml = options.project.join("Cargo.toml");
    if !cargo_toml.is_file() {
        return Err(CliError::NotFound {
            what: format!(
                "no Cargo.toml in {} — `clwd build` takes the directory of the app to build",
                options.project.display()
            ),
        });
    }

    let manifest_path = options.project.join(manifest::FILENAME);
    let source = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => Source::parse(&text)?,
        // Absent is valid and means "everything default", which deny-by-default
        // makes safe: no egress, no secrets, no storage.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Source::default(),
        Err(e) => {
            return Err(CliError::ManifestInvalid {
                what: format!("{} could not be read: {e}", manifest_path.display()),
            });
        }
    };

    if let Ok(text) = std::fs::read_to_string(&cargo_toml) {
        warnings.extend(cargo::profile_warnings(&text));
    }

    if let Some(p) = progress {
        p.step("building the component");
    }
    let component = cargo::compile(
        &options.project,
        progress,
        options.output.as_ref(),
        options.cancel.as_ref(),
    )?;

    let toolchain = Toolchain::detect();
    if options.optimize && !toolchain.has_wasm_tools() {
        warnings.push(Toolchain::wasm_tools_warning());
    }

    let component = if options.optimize {
        let stripped_path = component.with_extension("stripped.wasm");
        if let Some(p) = progress {
            p.step("stripping custom sections");
        }
        if toolchain.strip(&component, &stripped_path)? {
            stripped_path
        } else {
            component
        }
    } else {
        component
    };
    let stripped = options.optimize && toolchain.has_wasm_tools();

    let bytes = std::fs::read(&component).map_err(|e| CliError::BuildFailed {
        step: "reading the component",
        detail: format!("{}: {e}", component.display()),
    })?;
    let component_size = ComponentSize::from_bytes(bytes.len() as u64);

    let name = component
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app.wasm")
        .to_owned();

    let members = vec![File {
        path: name.clone(),
        data: bytes,
        executable: false,
    }];
    Source::reject_manifest_as_member(&members.iter().map(|f| f.path.clone()).collect::<Vec<_>>())?;

    let capabilities = source.capabilities()?;
    let encoded = clwd_bundle::build(
        source.runtime()?,
        source.entrypoint(&name),
        capabilities.clone(),
        members,
    )?;

    let out = options
        .out
        .clone()
        .unwrap_or_else(|| component.with_extension(BUNDLE_EXT));

    // The digest comparison described in the module docs. Reading an existing
    // bundle to compare is cheaper than rewriting one, and leaving the file
    // untouched preserves its mtime for anything watching it.
    let unchanged = std::fs::read(&out).is_ok_and(|existing| existing == encoded.bytes);
    if options.cancel.as_ref().is_some_and(Cancel::is_cancelled) {
        return Err(CliError::BuildFailed {
            step: "writing the bundle",
            detail: "the build was cancelled before its bundle was written".to_owned(),
        });
    }
    if !unchanged {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CliError::BuildFailed {
                step: "writing the bundle",
                detail: format!("{}: {e}", parent.display()),
            })?;
        }
        // Written beside the bundle and renamed into place, so a build stopped
        // midway leaves the previous bundle or none, never half of one.
        let partial = out.with_extension(format!("{BUNDLE_EXT}.partial"));
        std::fs::write(&partial, &encoded.bytes)
            .and_then(|()| std::fs::rename(&partial, &out))
            .map_err(|e| {
                let _ = std::fs::remove_file(&partial);
                CliError::BuildFailed {
                    step: "writing the bundle",
                    detail: format!("{}: {e}", out.display()),
                }
            })?;
    }

    let bundle_size = BundleSize::from_bytes(encoded.bytes.len() as u64);
    check(
        component_size.verdict(),
        "component",
        component_size.bytes(),
    )?;
    check(bundle_size.verdict(), "bundle", bundle_size.bytes())?;
    if let Verdict::Warn { threshold } = component_size.verdict() {
        warnings.push(format!(
            "component is {component_size}, over the {} warn threshold",
            size::ComponentSize::from_bytes(threshold)
        ));
    }
    if let Verdict::Warn { threshold } = bundle_size.verdict() {
        warnings.push(format!(
            "bundle is {bundle_size}, over the {} warn threshold",
            size::BundleSize::from_bytes(threshold)
        ));
    }

    if let Some(p) = progress {
        p.finish();
    }

    Ok(Built {
        bundle: out,
        digest: encoded.digest.to_string(),
        component_size,
        bundle_size,
        stripped,
        unchanged,
        capabilities: describe(&capabilities),
        warnings,
    })
}

impl std::fmt::Display for Built {
    /// What a person reads after a build.
    ///
    /// The capability block is unconditional. An author who granted nothing
    /// should see that as plainly as one who granted something — deny-by-default
    /// is invisible until it bites at runtime otherwise, and "no output" is not
    /// a statement anyone can act on.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for w in &self.warnings {
            writeln!(f, "warning: {w}")?;
        }

        writeln!(f, "{}", self.bundle.display())?;
        writeln!(f, "  digest:    {}", self.digest)?;
        writeln!(
            f,
            "  component: {}{}",
            self.component_size,
            if self.stripped { " (stripped)" } else { "" }
        )?;
        writeln!(f, "  bundle:    {}", self.bundle_size)?;
        if self.unchanged {
            writeln!(f, "  unchanged: the existing bundle was already current")?;
        }

        writeln!(f, "  granted:")?;
        for line in &self.capabilities {
            writeln!(f, "    {line}")?;
        }
        Ok(())
    }
}

/// Turn a fatal verdict into an error naming both numbers.
fn check(verdict: Verdict, what: &'static str, actual: u64) -> Result<(), CliError> {
    if let Verdict::Exceeded { ceiling } = verdict {
        return Err(CliError::BuildFailed {
            step: "size check",
            detail: format!("the {what} is {actual} bytes, over the {ceiling} byte ceiling"),
        });
    }
    Ok(())
}

/// The path a bundle is written to when `--out` is not given.
///
/// Public so a caller can predict it without running a build.
#[must_use]
pub fn default_output(component: &Path) -> PathBuf {
    component.with_extension(BUNDLE_EXT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clwd_bundle::{Capabilities, Limits};

    fn caps(egress: &[&str]) -> Capabilities {
        Capabilities {
            version: clwd_bundle::manifest::CAPABILITIES_VERSION,
            egress: egress.iter().map(|s| (*s).to_owned()).collect(),
            secrets: Vec::new(),
            storage_namespaces: Vec::new(),
            limits: Limits {
                cpu_ms: 5000,
                memory_mb: 128,
            },
        }
    }

    /// Deny-by-default has to be visible. An author granted nothing should read
    /// that as plainly as an author granted something.
    #[test]
    fn an_empty_capability_set_is_still_printed_in_full() {
        let d = describe(&caps(&[]));
        assert!(d.iter().any(|l| l == "egress: none"), "{d:?}");
        assert!(d.iter().any(|l| l == "secrets: none"), "{d:?}");
        assert!(d.iter().any(|l| l == "storage: none"), "{d:?}");
        assert!(d.iter().any(|l| l.contains("5000 ms")), "{d:?}");
    }

    #[test]
    fn a_granted_capability_is_named_rather_than_counted() {
        let d = describe(&caps(&["api.example.com"]));
        assert!(
            d.iter().any(|l| l.contains("api.example.com")),
            "the grant itself must appear, not just its count: {d:?}"
        );
    }

    #[test]
    fn a_size_over_the_ceiling_fails_the_build_naming_both_numbers() {
        let err = check(Verdict::Exceeded { ceiling: 100 }, "component", 250)
            .expect_err("over the ceiling must fail");
        let msg = err.to_string();
        assert!(msg.contains("250"), "actual size missing: {msg}");
        assert!(msg.contains("100"), "the ceiling missing: {msg}");
    }

    #[test]
    fn a_warning_verdict_does_not_fail_the_build() {
        assert!(check(Verdict::Warn { threshold: 10 }, "component", 20).is_ok());
        assert!(check(Verdict::Ok, "component", 1).is_ok());
    }

    #[test]
    fn the_default_output_sits_beside_the_component() {
        let out = default_output(Path::new("/t/wasm32-wasip2/release/app.wasm"));
        assert_eq!(out, Path::new("/t/wasm32-wasip2/release/app.clwd"));
    }
}
