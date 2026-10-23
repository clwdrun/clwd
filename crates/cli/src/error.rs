//! The error type at the library boundary.
//!
//! Every variant carries a stable, namespaced code. Those codes are a
//! compatibility surface: `--json` emits them, and #31 pins them. Renaming one
//! is a breaking change, so they are declared in exactly one place — the macro
//! invocation below — and a test holds the declaration to the variant list.
//!
//! # `CliError::Runtime` — delegation, and why the code is not re-spelled
//!
//! A failure the sandbox or the bundle format decided is surfaced with its own
//! code intact. **That pass-through is the contract, not a convenience.** A
//! caller told `bundle.member_digest_mismatch` can rebuild their bundle; one
//! told `cli.internal` learns only that something, somewhere, went wrong. The
//! prefix names the subsystem that made the decision — a guarantee
//! `docs/error-codes.md` publishes and the platform mirrors across the seam.
//!
//! **Two delegating variants, and the second one is load-bearing.** Until #28
//! there was one: `bundle.*` codes arrived through `clwd_runtime::Error::Bundle`,
//! and a variant wrapping `clwd_bundle::Error` directly was refused as a second
//! path to a code no caller could distinguish.
//!
//! `clwd build` breaks that. It encodes a bundle with no sandbox anywhere in the
//! picture, so routing its failures through `clwd_runtime::Error` would make
//! `build` require `feature = "run"` — which is the feature that carries
//! wasmtime. A command that compiles a component would then drag a runtime into
//! the CLI to report that a manifest was malformed, and `cli-deps` would fail on
//! it, correctly.
//!
//! So `Bundle` is now direct and unconditional, and `Runtime` stays behind
//! `run`. The original worry does not survive contact with what a caller
//! actually sees: both paths emit the identical code string, and the code is the
//! contract. What matters is that a `bundle.*` code is never rewritten into a
//! `cli.*` one, and two variants preserve that as well as one did.

/// Declares `CliError` and, from the same input, the list of every code it can
/// produce. A variant cannot be added without a code, which is what makes the
/// exhaustiveness test in this module able to tell the truth.
///
/// Two sections, mirroring `clwd_runtime::Error`: `delegating` variants borrow
/// a nested error's catalogue, `owned` variants carry a `cli.*` literal.
///
/// # Two non-obvious properties of the attribute handling
///
/// **`#[error(transparent)]` is written by this macro rather than accepted from
/// the invocation.** An attribute passed through a `$(#[$a:meta])*` fragment
/// carries the macro's hygiene context, so thiserror resolves `transparent` as
/// an identifier in *this* scope and fails with ``cannot find value
/// `transparent` ``. It is also the better contract: a delegating variant is
/// transparent by construction, so making it an input offers a choice with one
/// correct answer.
///
/// **Attribute forwarding to the generated `match` arms is selective, and has to
/// be.** `#[cfg]` must reach the arms: that is what lets the delegating section
/// sit behind `#[cfg(feature = "run")]`, so the variant and its `code()` and
/// `hint()` arms, and the hand-written `retryable()` arm, appear and disappear together. Forwarding the
/// arm attribute on the variant alone leaves arms naming a variant that does not
/// exist, and the crate stops compiling with the feature off.
///
/// `#[error]` must *not* reach them — it is a thiserror variant attribute, and
/// on a match arm it fails with the same ``cannot find attribute `error` `` that
/// the hygiene note above describes. Doc comments must not either: on a match
/// arm they are an unused-doc warning, which `-D warnings` turns into a failure.
/// Hence the `owned` arms forward nothing and the `delegating` section carries
/// only its gate, with its prose on the module. This is easy to get wrong —
/// it was got wrong once already, two edits after the hygiene note was written.
macro_rules! define_cli_error {
    (
        // Braced so the two repetitions are bounded: without a delimiter the
        // parser cannot tell the `owned` keyword from another variant name.
        delegating {
            $(
                $(#[$dattr:meta])*
                $dvariant:ident { source: $dty:ty };
            )*
        }
        owned {
            $(
                $(#[$attr:meta])*
                $variant:ident { $($field:tt)* } => $code:literal, $hint:literal;
            )*
        }
    ) => {
        #[derive(Debug, thiserror::Error)]
        #[non_exhaustive]
        pub enum CliError {
            $( $(#[$dattr])* #[error(transparent)] $dvariant { source: $dty }, )*
            $( $(#[$attr])* $variant { $($field)* }, )*
        }

        impl CliError {
            /// The stable code for this error, as emitted by `--json`.
            ///
            /// May be from another namespace. A delegating variant returns its
            /// source's code **unchanged** — see the module documentation.
            ///
            /// Returns `&str` rather than `&'static str`: the `Platform`
            /// variant's code arrives from the wire at runtime, so it is
            /// borrowed from `self` rather than a program literal. Every other
            /// arm returns a `'static` string, which coerces.
            pub fn code(&self) -> &str {
                match self {
                    $( $(#[$dattr])* Self::$dvariant { source } => source.code(), )*
                    $( Self::$variant { .. } => $code, )*
                }
            }

            /// A short, actionable next step.
            ///
            /// Every variant has one. In the JSON envelope this field *is* the
            /// recovery a caller sees, and a null hint is a dead end for exactly
            /// the person least able to work out what to do next. A variant with
            /// nothing useful to say here is usually a variant that is too broad
            /// to be actionable at all.
            ///
            /// A delegating variant forwards its source's hint, as it forwards
            /// its code: the crate that decides a failure says how to recover
            /// from it. An owned variant's hint is a literal in the invocation
            /// below, never a format string, so it cannot carry error data.
            ///
            /// The `Option` stays in the signature rather than becoming
            /// `&'static str`: `hint` is `null`-able in the published envelope,
            /// a platform error may arrive without one, and narrowing the type
            /// here would make that shape unrepresentable.
            pub fn hint(&self) -> Option<&str> {
                match self {
                    // `.into()` takes both shapes: `clwd_runtime::Error` and
                    // `clwd_bundle::Error` return `&'static str`, while a
                    // platform error's hint is already an `Option`.
                    $( $(#[$dattr])* Self::$dvariant { source } => source.hint().into(), )*
                    $( Self::$variant { .. } => Some($hint), )*
                }
            }
        }

        /// Every `cli.*` code, generated from the same input as the enum.
        /// Public because `docs/error-codes.md` is built from it — a catalogue
        /// only the tests can see cannot be published.
        ///
        /// Delegated codes are deliberately absent: they belong to the crate
        /// that defines them and are catalogued there. This list is exactly what
        /// this crate is responsible for.
        pub const CODES: &[&str] = &[$($code),*];

        /// Every `cli.*` and `net.*` code paired with its hint, generated from
        /// the same input as [`CliError::hint`]. Public because the error-code
        /// table is built from it.
        pub const HINTS: &[(&str, &str)] = &[$(($code, $hint)),*];
    };
}

define_cli_error! {
    delegating {
        // Documented on the module rather than here: every attribute in this
        // section is forwarded to the generated `match` arms so the gate reaches
        // them, and a doc comment on a match arm is an unused-doc warning.
        #[cfg(feature = "run")]
        Runtime { source: clwd_runtime::Error };

        Bundle { source: clwd_bundle::Error };

        // A failure the platform decided, carrying its shared envelope's code
        // verbatim. Ungated: publishing needs no sandbox, so the network client
        // is unconditional and so is the error it produces. Its `code()` is a
        // runtime string, which is why `CliError::code` returns `&str`.
        Platform { source: crate::platform::PlatformError };
    }

    owned {
    /// A value clap accepted on shape but a command rejected on meaning.
    #[error("invalid argument: {what}")]
    InvalidArgument { what: String } => "cli.invalid_argument",
        "run `clwd <command> --help` for accepted values";

    /// A named thing — app, secret, bundle — that does not exist.
    #[error("not found: {what}")]
    NotFound { what: String } => "cli.not_found",
        "check the name for typos, and run the matching `list` command to see what exists";

    /// The command exists in the dispatch table but has no body yet.
    #[error("`clwd {command}` is not implemented yet")]
    NotImplemented { command: &'static str } => "cli.not_implemented",
        "track progress at https://github.com/clwdrun/clwd/issues";

    /// Local configuration is missing, unreadable, or self-contradictory.
    #[error("configuration error: {what}")]
    Config { what: String } => "cli.config",
        "check the config file the message names; delete it to start from defaults";

    /// The project's `clwd.toml` is malformed, contradictory, or grants
    /// something the format does not accept.
    ///
    /// Distinct from `cli.config` because the recovery differs. `cli.config`
    /// can suggest deleting the file to fall back to defaults; doing that to a
    /// manifest discards the author's capability grants, and for the
    /// member-collision case it is not the fix at all.
    #[error("{what}")]
    ManifestInvalid { what: String } => "cli.manifest_invalid",
        "check clwd.toml against the schema in docs/spec/bundle-v1.md §5";

    /// A tool the build needs is absent, or present and unrunnable.
    // The field is `cause`, not `source`: thiserror treats a field named
    // `source` as the error source and requires it to implement `Error`. This
    // one is the operating system's message, already rendered.
    #[error("{tool} could not be run: {cause}\ninstall it with: {install}")]
    ToolchainMissing {
        tool: &'static str,
        install: &'static str,
        cause: String,
    } => "cli.toolchain_missing",
        // The install command is in the message rather than here: it differs
        // per tool, and a hint is one static string per code.
        "install the tool the message names, then run the command again";

    /// A build step ran and failed. The step names which one, so a failure
    /// deep in a subprocess does not read as a defect in clwd.
    #[error("{step} failed: {detail}")]
    BuildFailed {
        step: &'static str,
        detail: String,
    } => "cli.build_failed",
        "Fix the first error in the build output above, then run `clwd build` again.";

    /// Another deploy of the same project is running. Refused rather than
    /// queued: two deploys interleaving their build and publish steps could
    /// publish either one's bundle last.
    #[error("another deploy of {project} is in progress")]
    DeployInProgress { project: String } => "cli.deploy_in_progress",
        "wait for the other deploy of this project to finish, then run it again";

    /// A network-layer failure with no HTTP response: connection refused, timed
    /// out, TLS handshake failed. The one publish failure the platform never
    /// gets to describe, because nothing reached it — so unlike a delegated
    /// `control-plane.*` code, this one is the CLI's own. `retryable` follows
    /// the transport rather than being fixed: a timeout or a refused connection
    /// may succeed on a later attempt.
    #[error("could not reach the platform: {detail}")]
    Net {
        detail: String,
        retryable: bool,
    } => "net.unreachable",
        "check the platform URL and your network connection, then try again";

    /// Last resort only. Never returned where a specific variant fits: an
    /// `anyhow` source inside a library error type is tolerable as a genuine
    /// fallback and corrosive as a default.
    #[error("internal error: {source}")]
    Internal {
        #[source]
        source: anyhow::Error,
    } => "cli.internal",
        "Report this at https://github.com/clwdrun/clwd/issues with the command that produced it.";
    }
}

/// So `?` works on a bundle operation without a `map_err` at every call site.
///
/// Deliberately not written for `clwd_runtime::Error`: that variant is behind
/// `feature = "run"`, and a `From` impl that appears and disappears with a
/// feature is a trap for anyone reading a call site with the feature on.
impl From<clwd_bundle::Error> for CliError {
    fn from(source: clwd_bundle::Error) -> Self {
        Self::Bundle { source }
    }
}

impl CliError {
    /// Whether retrying the same command unchanged could plausibly succeed.
    ///
    /// Every arm is spelled out rather than collapsed into a default so that
    /// adding a variant forces a decision here instead of inheriting `false`.
    /// The transient classes arrive with the issues that return them: `net.*`
    /// carries a transport-derived flag, and a platform `429` forwards the
    /// envelope's `retryable: true`.
    pub fn retryable(&self) -> bool {
        match self {
            // Forwarded, not guessed: the crate that owns the code decides
            // whether it is retryable, per variant, and this one repeats it.
            #[cfg(feature = "run")]
            Self::Runtime { source } => source.retryable(),
            // Forwarded, not guessed. `clwd_bundle::Error` does expose
            // `retryable()`, so the crate that owns the code makes the claim and
            // this one repeats it. If a `bundle.*` code ever becomes retryable,
            // that decision lands here without an edit.
            Self::Bundle { source } => source.retryable(),
            // Forwarded from the platform envelope — a `429` says `true`, the
            // rest say `false`, and the platform owns that decision.
            Self::Platform { source } => source.retryable(),
            // Carried on the variant: set from the transport at the point of
            // failure (a timeout or a refused connection may succeed later).
            Self::Net { retryable, .. } => *retryable,
            // The other deploy ends on its own; the same command then runs.
            Self::DeployInProgress { .. } => true,
            Self::InvalidArgument { .. }
            | Self::NotFound { .. }
            | Self::NotImplemented { .. }
            | Self::Config { .. }
            | Self::ManifestInvalid { .. }
            | Self::ToolchainMissing { .. }
            | Self::BuildFailed { .. }
            | Self::Internal { .. } => false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// One sample per variant, written by hand. `CODES` is generated
    /// from the enum definition, so the equality assertion below fails the
    /// moment a variant is added to the enum and forgotten here.
    pub(crate) fn samples() -> Vec<CliError> {
        vec![
            CliError::InvalidArgument { what: "x".into() },
            CliError::NotFound { what: "x".into() },
            CliError::NotImplemented { command: "x" },
            CliError::Config { what: "x".into() },
            CliError::ManifestInvalid { what: "x".into() },
            CliError::ToolchainMissing {
                tool: "x",
                install: "x",
                cause: "x".into(),
            },
            CliError::BuildFailed {
                step: "x",
                detail: "x".into(),
            },
            CliError::DeployInProgress {
                project: "x".into(),
            },
            CliError::Net {
                detail: "x".into(),
                retryable: false,
            },
            CliError::Internal {
                source: anyhow::anyhow!("x"),
            },
        ]
    }

    #[test]
    fn codes_are_unique() {
        let unique: BTreeSet<&str> = CODES.iter().copied().collect();
        assert_eq!(unique.len(), CODES.len(), "duplicate code in {CODES:?}");
    }

    #[test]
    fn codes_are_exhaustive_over_variants() {
        // Bound rather than chained: `code()` borrows the error, so the `Vec`
        // must outlive the `&str`s collected from it.
        let samples = samples();
        let produced: BTreeSet<&str> = samples.iter().map(CliError::code).collect();
        let declared: BTreeSet<&str> = CODES.iter().copied().collect();
        assert_eq!(
            produced, declared,
            "a variant is missing from `samples()` or from the enum"
        );
    }

    #[test]
    fn every_code_has_a_hint() {
        let hinted: Vec<&str> = HINTS.iter().map(|(code, _)| *code).collect();
        assert_eq!(hinted, CODES, "HINTS and CODES disagree");
        for (code, hint) in HINTS {
            assert!(!hint.trim().is_empty(), "{code} has an empty hint");
        }
        for err in samples() {
            let published = HINTS
                .iter()
                .find(|(c, _)| *c == err.code())
                .map(|(_, h)| *h);
            assert_eq!(err.hint(), published, "{}", err.code());
        }
    }

    /// A hint never carries error data, so it never needs a placeholder.
    #[test]
    fn no_hint_is_a_format_string() {
        for (code, hint) in HINTS {
            assert!(
                !hint.contains(['{', '}']),
                "{code}: a hint must not interpolate: {hint:?}"
            );
        }
    }

    #[test]
    fn codes_are_namespaced_and_lowercase() {
        for code in CODES {
            let (ns, rest) = code
                .split_once('.')
                .unwrap_or_else(|| panic!("{code} is not namespaced"));
            assert!(
                !ns.is_empty() && !rest.is_empty(),
                "{code} has an empty half"
            );
            assert_eq!(*code, code.to_lowercase(), "{code} is not lowercase");
        }
    }
}
