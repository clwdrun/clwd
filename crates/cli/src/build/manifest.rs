//! `clwd.toml` as an author writes it, and how it becomes the one that ships.
//!
//! # One name, two schemas
//!
//! The file an author writes and the file inside the bundle are both called
//! `clwd.toml`, and that is deliberate: it is the same document, before and
//! after the build fills in what only the build can know. The author states
//! intent — runtime, entrypoint, capabilities. `build` adds `[[members]]` with
//! a digest and size per file, and `schema_version`.
//!
//! The two schemas differ in exactly one way, and it matters. Spec §5.2 gives
//! the *bundle* manifest no optional keys at all: "The manifest has exactly one
//! shape: no optional keys, and therefore no key whose absence has to be
//! assigned a meaning." The *source* manifest is almost entirely optional, so
//! `build` must materialise every default rather than pass an absence through.
//! An empty `clwd.toml` is valid and produces a complete, fully-specified
//! bundle manifest.
//!
//! # Unknown keys are rejected, here as in the spec
//!
//! §7.4 rejects unknown keys in the bundle manifest. The same rule applies to
//! the source, for a sharper reason: the keys here are mostly capability
//! grants, and a misspelled grant that parses as "unknown, ignored" is a
//! permission the author believes they wrote and did not get. Deny-by-default
//! is only honest if a typo is loud.

use clwd_bundle::{Capabilities, Limits, Runtime};
use serde::Deserialize;

use crate::error::CliError;

/// The bundle's manifest filename, from `clwd_bundle::manifest::FILENAME`.
///
/// Named here rather than imported so the collision check below reads as what
/// it is. See [`Source::reject_manifest_as_member`].
pub const FILENAME: &str = "clwd.toml";

/// Applied when `[capabilities.limits]` says nothing. Matches the spec's
/// worked example in §5.2, so the documented sample and the default agree.
const DEFAULT_CPU_MS: u64 = 5_000;
/// Applied when `[capabilities.limits]` says nothing.
const DEFAULT_MEMORY_MB: u64 = 128;

/// The manifest an author writes.
///
/// Every field is optional; see the module docs for why the bundle's is not.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Source {
    runtime: Option<String>,
    app: Option<AppSection>,
    capabilities: Option<CapabilitiesSection>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AppSection {
    entrypoint: Option<String>,
    /// The app's name, which is what its secrets are scoped to.
    ///
    /// CLI-side only: it is not a bundle-manifest field, because the bundle
    /// format assigns no meaning to an app's name and adding one would be a
    /// spec change for something only local tooling needs.
    name: Option<String>,
    /// The app's permanent host label, which `clwd deploy` creates it under.
    ///
    /// CLI-side only, like `name`: the bundle is the same whichever app it is
    /// published to.
    host_label: Option<String>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CapabilitiesSection {
    egress: Option<Vec<String>>,
    secrets: Option<Vec<String>>,
    storage_namespaces: Option<Vec<String>>,
    limits: Option<LimitsSection>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LimitsSection {
    cpu_ms: Option<u64>,
    memory_mb: Option<u64>,
}

fn invalid(what: impl Into<String>) -> CliError {
    CliError::ManifestInvalid { what: what.into() }
}

/// Whether `s` is a DNS label per RFC 1123, lowercase only: 1 to 63 of
/// `a-z`, `0-9` and `-`, not starting or ending with `-`.
///
/// Only the syntax. Whether the org may use the label — taken, or retired with
/// a deleted app — is the platform's to decide.
#[must_use]
pub fn is_host_label(s: &str) -> bool {
    (1..=63).contains(&s.len())
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Normalize one capability array: reject empty elements and duplicates, sort.
///
/// Sorting rather than demanding the author sort: §5.3 requires one grant set
/// to have one spelling in the *bundle*, and that is a canonicalization the
/// build can perform. Duplicates are rejected rather than deduplicated, because
/// a repeated grant is more likely a mistake than an intention, and silently
/// collapsing it would hide the mistake.
fn normalize_grants(field: &str, mut values: Vec<String>) -> Result<Vec<String>, CliError> {
    if let Some(pos) = values.iter().position(|v| v.is_empty()) {
        return Err(invalid(format!(
            "capabilities.{field}[{pos}] is an empty string; every grant must name something"
        )));
    }
    values.sort_unstable();
    if let Some(dup) = values.windows(2).find(|w| w[0] == w[1]) {
        return Err(invalid(format!(
            "capabilities.{field} lists {:?} more than once",
            dup[0]
        )));
    }
    Ok(values)
}

impl Source {
    /// Parse a source manifest.
    ///
    /// Strict: unknown keys are an error, not a warning. See the module docs.
    pub fn parse(text: &str) -> Result<Self, CliError> {
        toml::from_str(text).map_err(|e| {
            // `toml`'s message already carries the line, column and the
            // offending key. Restating it would lose that precision.
            invalid(format!("{FILENAME} is not valid: {e}"))
        })
    }

    /// The declared runtime, defaulting to `wasm`.
    pub fn runtime(&self) -> Result<Runtime, CliError> {
        match self.runtime.as_deref() {
            None | Some("wasm") => Ok(Runtime::Wasm),
            Some("microvm") => Ok(Runtime::MicroVm),
            Some(other) => Err(invalid(format!(
                "runtime = {other:?} is not a runtime; expected \"wasm\" or \"microvm\""
            ))),
        }
    }

    /// The declared entrypoint, or `fallback` when none is declared.
    ///
    /// The fallback is the built artifact's own name, which the caller knows
    /// and this type does not.
    #[must_use]
    pub fn entrypoint<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.app
            .as_ref()
            .and_then(|a| a.entrypoint.as_deref())
            .unwrap_or(fallback)
    }

    /// The app's declared name, if it has one.
    ///
    /// No fallback to the directory name, deliberately: secrets are scoped to
    /// this, and two projects sharing a namespace should be something an author
    /// chose rather than a consequence of where a repository was cloned.
    #[must_use]
    pub fn app_name(&self) -> Option<&str> {
        self.app.as_ref().and_then(|a| a.name.as_deref())
    }

    /// The app's declared host label, if it has one.
    ///
    /// No fallback, for the reason `app_name` has none, and a sharper one: a
    /// label is permanent once an app is created under it.
    #[must_use]
    pub fn host_label(&self) -> Option<&str> {
        self.app.as_ref().and_then(|a| a.host_label.as_deref())
    }

    /// The capability set to seal into the bundle, defaults applied.
    ///
    /// Deny-by-default per #33: an undeclared app gets no egress, no secrets
    /// and no storage namespaces. Those are three explicit empty arrays in the
    /// output, never absences — §5.3 requires a grant of nothing to say so.
    pub fn capabilities(&self) -> Result<Capabilities, CliError> {
        let caps = self.capabilities.as_ref();
        let take = |f: fn(&CapabilitiesSection) -> Option<&Vec<String>>| {
            caps.and_then(f).cloned().unwrap_or_default()
        };

        let limits = caps.and_then(|c| c.limits.as_ref());
        let cpu_ms = limits.and_then(|l| l.cpu_ms).unwrap_or(DEFAULT_CPU_MS);
        let memory_mb = limits
            .and_then(|l| l.memory_mb)
            .unwrap_or(DEFAULT_MEMORY_MB);

        Ok(Capabilities {
            version: clwd_bundle::manifest::CAPABILITIES_VERSION,
            egress: normalize_grants("egress", take(|c| c.egress.as_ref()))?,
            secrets: normalize_grants("secrets", take(|c| c.secrets.as_ref()))?,
            storage_namespaces: normalize_grants(
                "storage_namespaces",
                take(|c| c.storage_namespaces.as_ref()),
            )?,
            limits: Limits { cpu_ms, memory_mb },
        })
    }

    /// Reject a member named `clwd.toml` before the encoder has to.
    ///
    /// The encoder does reject it — §5.4, `bundle.manifest_self_listed` — but
    /// that error arrives from deep inside the bundle format and describes the
    /// archive's invariant rather than the author's mistake. The mistake is
    /// almost always an asset glob that swept the project root, and the fix is
    /// to exclude one file. Saying that here costs a few lines and turns a
    /// puzzling failure into an instruction.
    pub fn reject_manifest_as_member(paths: &[String]) -> Result<(), CliError> {
        if paths.iter().any(|p| p == FILENAME) {
            return Err(invalid(format!(
                "{FILENAME} cannot also be a bundle member: the build writes the \
                 bundle's own {FILENAME}, so including your project's copy would \
                 mean two files claiming the same path. Exclude it from the files \
                 you are bundling."
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_manifest_is_valid_and_yields_a_fully_specified_bundle() {
        let s = Source::parse("").expect("an empty manifest is valid");
        let caps = s.capabilities().expect("defaults apply");
        assert_eq!(s.runtime().expect("default runtime"), Runtime::Wasm);
        assert_eq!(s.entrypoint("app.wasm"), "app.wasm");
        assert_eq!(caps.limits.cpu_ms, DEFAULT_CPU_MS);
        assert_eq!(caps.limits.memory_mb, DEFAULT_MEMORY_MB);
    }

    /// Deny-by-default, spelled out. Three empty arrays, not three absences.
    #[test]
    fn an_undeclared_app_gets_nothing_and_says_so_explicitly() {
        let caps = Source::parse("")
            .expect("valid")
            .capabilities()
            .expect("defaults");
        assert!(caps.egress.is_empty());
        assert!(caps.secrets.is_empty());
        assert!(caps.storage_namespaces.is_empty());
    }

    /// The whole point of `deny_unknown_fields`: a misspelled grant must not
    /// parse as "unknown, ignored". An author who writes `egres` and gets a
    /// silently ungranted capability has been lied to.
    #[test]
    fn a_misspelled_capability_key_is_rejected_rather_than_ignored() {
        let err = Source::parse("[capabilities]\negres = [\"example.com\"]\n")
            .expect_err("a misspelled key must not be accepted");
        assert_eq!(err.code(), "cli.manifest_invalid");
        assert!(
            err.to_string().contains("egres"),
            "the message must name the offending key, got: {err}"
        );
    }

    #[test]
    fn an_unknown_top_level_key_is_rejected() {
        let err = Source::parse("rutnime = \"wasm\"\n").expect_err("typo must be rejected");
        assert_eq!(err.code(), "cli.manifest_invalid");
    }

    #[test]
    fn grants_are_sorted_so_one_grant_set_has_one_spelling() {
        let caps = Source::parse("[capabilities]\negress = [\"z.example\", \"a.example\"]\n")
            .expect("valid")
            .capabilities()
            .expect("valid grants");
        assert_eq!(caps.egress, ["a.example", "z.example"]);
    }

    /// Rejected, not deduplicated: a repeated grant is more likely a mistake
    /// than an intention, and collapsing it silently would hide the mistake.
    #[test]
    fn a_duplicated_grant_is_rejected_rather_than_collapsed() {
        let err = Source::parse("[capabilities]\nsecrets = [\"k\", \"k\"]\n")
            .expect("parses")
            .capabilities()
            .expect_err("a duplicate must be rejected");
        assert!(err.to_string().contains("more than once"), "got: {err}");
    }

    #[test]
    fn an_empty_grant_string_is_rejected() {
        let err = Source::parse("[capabilities]\negress = [\"\"]\n")
            .expect("parses")
            .capabilities()
            .expect_err("an empty grant must be rejected");
        assert!(err.to_string().contains("empty string"), "got: {err}");
    }

    #[test]
    fn an_unrecognised_runtime_is_rejected_by_name() {
        let err = Source::parse("runtime = \"jvm\"\n")
            .expect("parses")
            .runtime()
            .expect_err("jvm is not a runtime");
        assert!(err.to_string().contains("jvm"), "got: {err}");
    }

    #[test]
    fn a_host_label_is_an_rfc_1123_label() {
        for good in ["a", "notes", "my-app-2", "0day", &"a".repeat(63)] {
            assert!(is_host_label(good), "{good:?} is a label");
        }
        for bad in [
            "",
            "-notes",
            "notes-",
            "Notes",
            "my_app",
            "my.app",
            "caf\u{e9}",
            &"a".repeat(64),
        ] {
            assert!(!is_host_label(bad), "{bad:?} is not a label");
        }
    }

    #[test]
    fn a_host_label_is_read_from_the_app_section() {
        let s = Source::parse("[app]\nhost_label = \"notes\"\n").expect("valid");
        assert_eq!(s.host_label(), Some("notes"));
        assert_eq!(Source::default().host_label(), None);
    }

    #[test]
    fn a_declared_entrypoint_overrides_the_fallback() {
        let s = Source::parse("[app]\nentrypoint = \"main.wasm\"\n").expect("valid");
        assert_eq!(s.entrypoint("app.wasm"), "main.wasm");
    }

    /// The collision the CLI must name itself rather than let the encoder
    /// surface as `bundle.manifest_self_listed`.
    #[test]
    fn a_project_manifest_among_the_members_is_refused_with_the_fix() {
        let members = vec!["app.wasm".to_owned(), FILENAME.to_owned()];
        let err = Source::reject_manifest_as_member(&members)
            .expect_err("clwd.toml must not be a member");
        assert_eq!(err.code(), "cli.manifest_invalid");
        let msg = err.to_string();
        assert!(
            msg.contains("Exclude it from the files"),
            "the message must state the fix, got: {msg}"
        );
        assert!(
            !msg.contains("manifest_self_listed"),
            "the CLI should name the author's mistake, not the encoder's invariant"
        );
    }

    #[test]
    fn members_without_the_manifest_are_accepted() {
        let members = vec!["app.wasm".to_owned(), "static/style.css".to_owned()];
        assert!(Source::reject_manifest_as_member(&members).is_ok());
    }

    /// Limits are independently defaulted: declaring one must not silently
    /// reset the other to the spec minimum.
    #[test]
    fn declaring_one_limit_leaves_the_other_at_its_default() {
        let caps = Source::parse("[capabilities.limits]\ncpu_ms = 20000\n")
            .expect("valid")
            .capabilities()
            .expect("valid");
        assert_eq!(caps.limits.cpu_ms, 20_000);
        assert_eq!(caps.limits.memory_mb, DEFAULT_MEMORY_MB);
    }
}
