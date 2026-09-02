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
/// sit behind `#[cfg(feature = "run")]`, so the variant and its `code()`,
/// `hint()` and `retryable()` arms appear and disappear together. Forwarding the
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
                $variant:ident { $($field:tt)* } => $code:literal;
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
            pub fn code(&self) -> &'static str {
                match self {
                    $( $(#[$dattr])* Self::$dvariant { source } => source.code(), )*
                    $( Self::$variant { .. } => $code, )*
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
    }

    owned {
    /// A value clap accepted on shape but a command rejected on meaning.
    #[error("invalid argument: {what}")]
    InvalidArgument { what: String } => "cli.invalid_argument";

    /// A named thing — app, secret, bundle — that does not exist.
    #[error("not found: {what}")]
    NotFound { what: String } => "cli.not_found";

    /// The command exists in the dispatch table but has no body yet.
    #[error("`clwd {command}` is not implemented yet")]
    NotImplemented { command: &'static str } => "cli.not_implemented";

    /// Local configuration is missing, unreadable, or self-contradictory.
    #[error("configuration error: {what}")]
    Config { what: String } => "cli.config";

    /// The project's `clwd.toml` is malformed, contradictory, or grants
    /// something the format does not accept.
    ///
    /// Distinct from `cli.config` because the recovery differs. `cli.config`
    /// can suggest deleting the file to fall back to defaults; doing that to a
    /// manifest discards the author's capability grants, and for the
    /// member-collision case it is not the fix at all.
    #[error("{what}")]
    ManifestInvalid { what: String } => "cli.manifest_invalid";

    /// Last resort only. Never returned where a specific variant fits: an
    /// `anyhow` source inside a library error type is tolerable as a genuine
    /// fallback and corrosive as a default.
    #[error("internal error: {source}")]
    Internal {
        #[source]
        source: anyhow::Error,
    } => "cli.internal";
    }
}

/// What a delegated error offers instead of a hint of its own.
///
/// A real next step rather than a null, which #31 established is a dead end for
/// the reader least able to work out what to do. It points at the table rather
/// than restating it, so it cannot drift.
///
/// Path rather than URL while the repository is private; this becomes a docs
/// URL when it goes public at MVP.
///
/// Ungated since #28. It was behind `run` when `Runtime` was the only variant
/// using it; `Bundle` is unconditional, so with the feature off there is still
/// something to delegate and the constant is still live.
const DELEGATED_HINT: &str = "see docs/error-codes.md for this code's meaning and how to recover";

impl CliError {
    /// A short, actionable next step.
    ///
    /// Every variant has one. In the JSON envelope this field *is* the recovery
    /// a caller sees, and a null hint is a dead end for exactly the person least
    /// able to work out what to do next. A variant with nothing useful to say
    /// here is usually a variant that is too broad to be actionable at all.
    ///
    /// The `Option` stays in the signature rather than becoming `&'static str`:
    /// `hint` is `null`-able in the published envelope, asserted by
    /// `json_error_keeps_a_null_hint_rather_than_dropping_the_key`, and
    /// narrowing the type here would make that shape unrepresentable if a
    /// future variant genuinely has no next step.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            // Delegated codes get a pointer to the table rather than a hint of
            // their own. There are 44 of them, and writing per-code advice here
            // would duplicate `docs/error-codes.md` into match arms that drift
            // from it — the table is generated precisely so there is one copy.
            //
            // Neither `clwd_runtime::Error` nor `clwd_bundle::Error` exposes a
            // `hint()` to forward. That is where the text belongs, and adding it
            // there is filed as follow-up work; this crate is papering over its
            // absence, not deciding it should be absent.
            #[cfg(feature = "run")]
            Self::Runtime { .. } => Some(DELEGATED_HINT),
            Self::Bundle { .. } => Some(DELEGATED_HINT),
            Self::InvalidArgument { .. } => Some("run `clwd <command> --help` for accepted values"),
            Self::NotFound { .. } => Some(
                "check the name for typos, and run the matching `list` command to see what exists",
            ),
            Self::NotImplemented { .. } => {
                Some("track progress at https://github.com/clwdrun/clwd/issues")
            }
            Self::Config { .. } => {
                Some("check the config file the message names; delete it to start from defaults")
            }
            Self::ManifestInvalid { .. } => {
                Some("check clwd.toml against the schema in docs/spec/bundle-v1.md §5")
            }
            Self::Internal { .. } => Some("this is a bug in clwd; please report it"),
        }
    }

    /// Whether retrying the same command unchanged could plausibly succeed.
    ///
    /// Every arm is spelled out rather than collapsed into a default so that
    /// adding a variant forces a decision here instead of inheriting `false`.
    /// Nothing is retryable yet — the transient classes (`net.*`, `runtime.*`)
    /// arrive with the issues that can actually return them.
    pub fn retryable(&self) -> bool {
        match self {
            // `clwd_bundle::Error::retryable` exists and is `false` for its whole
            // catalogue — a malformed bundle does not become well formed on a
            // second attempt. `clwd_runtime::Error` exposes no `retryable()` at
            // all, so there is nothing to forward for the six `runtime.*` codes.
            //
            // `false` rather than a guess. `runtime.deadline_exceeded` is
            // arguably retryable, but the crate that owns the code declined to
            // make that claim, and this crate inventing one on its behalf would
            // put a compatibility promise in the wrong place. The asymmetry is
            // deliberate and visible rather than papered over.
            #[cfg(feature = "run")]
            Self::Runtime { .. } => false,
            // Forwarded, not guessed. `clwd_bundle::Error` does expose
            // `retryable()`, so the crate that owns the code makes the claim and
            // this one repeats it. If a `bundle.*` code ever becomes retryable,
            // that decision lands here without an edit.
            Self::Bundle { source } => source.retryable(),
            Self::InvalidArgument { .. }
            | Self::NotFound { .. }
            | Self::NotImplemented { .. }
            | Self::Config { .. }
            | Self::ManifestInvalid { .. }
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
        let produced: BTreeSet<&str> = samples().iter().map(CliError::code).collect();
        let declared: BTreeSet<&str> = CODES.iter().copied().collect();
        assert_eq!(
            produced, declared,
            "a variant is missing from `samples()` or from the enum"
        );
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
