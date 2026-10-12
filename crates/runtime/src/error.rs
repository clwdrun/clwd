//! The runtime error type and its stable code namespace.
//!
//! Codes follow the pattern established for the CLI: the enum and the code list
//! are generated from one macro invocation, so a variant cannot ship without a
//! code, and a test holds the two to each other.
//!
//! # A code is not always `runtime.*`
//!
//! [`Error::code`] may return a code from another namespace, and that is
//! deliberate. When a bundle fails to load, the `bundle.*` code is preserved
//! verbatim rather than re-spelled as something like `runtime.bundle_invalid`.
//! A caller told `bundle.manifest_not_canonical` can fix the manifest; a caller
//! told `runtime.bundle_invalid` only learns that something, somewhere, was
//! wrong with a bundle.
//!
//! **The prefix is a guarantee, not a coincidence: it names the subsystem that
//! made the decision.** Consumers may rely on that.

use std::time::Duration;

use clwd_bundle::BundleRef;

/// Declares `Error`, its `code()` and `hint()`, and the codes and hints this
/// crate owns.
///
/// # Attributes are emitted here, not forwarded
///
/// `#[error(transparent)]` is written by this macro rather than accepted from
/// the invocation. That is not a style choice. An attribute passed through a
/// `$(#[$a:meta])*` fragment carries the macro's hygiene context, so thiserror
/// resolves `transparent` as an identifier in *this* scope and fails with
/// `cannot find value `transparent``. The same will happen to the next
/// attribute someone tries to forward the same way.
///
/// It is also the better contract: a delegating variant is transparent by
/// construction, so making it an input offers a choice with one correct answer.
///
/// Two sections, because the two kinds of variant differ in where their code
/// comes from — `delegating` variants borrow a nested error's catalogue, and
/// `owned` variants carry a `runtime.*` literal. Splitting them structurally
/// keeps the owned list honest: it is exactly the codes this crate is
/// responsible for.
///
/// # Hints are literals
///
/// Each owned variant carries one static sentence saying what to do next. It
/// is a string literal rather than a format string, so it cannot carry error
/// data: a trap message or a denied host is untrusted, and belongs in the
/// message rather than in the recovery text.
///
/// # Retryability is per variant
///
/// `retry: true` means the **same request**, sent again unchanged, can succeed
/// on a later attempt without anyone changing anything. Spelled on every owned
/// variant, so a new one cannot ship without the decision, and there is no
/// default arm to inherit one from.
macro_rules! define_error {
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
                $(#[$oattr:meta])*
                $ovariant:ident { $($ofield:tt)* } => $ocode:literal, retry: $oretry:literal, $ohint:literal;
            )*
        }
    ) => {
        #[derive(Debug, thiserror::Error)]
        #[non_exhaustive]
        pub enum Error {
            $( $(#[$dattr])* #[error(transparent)] $dvariant { source: $dty }, )*
            $( $(#[$oattr])* $ovariant { $($ofield)* }, )*
        }

        impl Error {
            /// The stable code for this failure.
            ///
            /// May be from another namespace — see the module documentation.
            pub fn code(&self) -> &'static str {
                match self {
                    $( Self::$dvariant { source } => source.code(), )*
                    $( Self::$ovariant { .. } => $ocode, )*
                }
            }

            /// What to do next: one imperative sentence naming the command or
            /// `clwd.toml` key to change. A delegating variant forwards its
            /// source's hint, as it forwards its code.
            pub fn hint(&self) -> &'static str {
                match self {
                    $( Self::$dvariant { source } => source.hint(), )*
                    $( Self::$ovariant { .. } => $ohint, )*
                }
            }

            /// Whether the same request, sent again unchanged, can succeed on a
            /// later attempt. A delegating variant forwards its source's answer.
            pub fn retryable(&self) -> bool {
                match self {
                    $( Self::$dvariant { source } => source.retryable(), )*
                    $( Self::$ovariant { .. } => $oretry, )*
                }
            }
        }

        /// Every code this crate owns, generated from the same input as the
        /// enum. Delegated codes belong to the crate that defines them and are
        /// catalogued there. Public because `docs/error-codes.md` is built from
        /// it — a catalogue only the tests can see cannot be published.
        pub const OWNED_CODES: &[&str] = &[$($ocode),*];

        /// Every owned code paired with its hint, generated from the same input
        /// as [`Error::hint`]. Public because the error-code table is built
        /// from it.
        pub const OWNED_HINTS: &[(&str, &str)] = &[$(($ocode, $ohint)),*];

        /// Every owned code paired with whether it is retryable, generated from
        /// the same input as [`Error::retryable`].
        pub const OWNED_RETRYABLE: &[(&str, bool)] = &[$(($ocode, $oretry)),*];
    };
}

define_error! {
    delegating {
        /// The bundle named by a `BundleRef` could not be loaded or validated.
        ///
        /// The `bundle.*` code survives intact — see the module documentation.
        Bundle { source: clwd_bundle::Error };
    }

    owned {
        /// No artifact is stored under this digest.
        ///
        /// Distinct from [`Error::Bundle`]: the bundle is absent, not invalid.
        #[error("no bundle stored under {bundle}")]
        BundleNotFound { bundle: BundleRef } => "runtime.bundle_not_found", retry: false,
            "Publish the app with `clwd publish`, or pass the digest from the `clwd build` you meant to run.";

        /// The caller's cold-start deadline (`SpawnOptions::deadline`) was
        /// already spent, or compilation outlasted it.
        ///
        /// Retryable: the deadline is the caller's and covers host load, so a
        /// later attempt can start inside it. A component that always compiles
        /// slower than the deadline is the exception, because a timed-out
        /// compilation is not kept.
        #[error("spawn exceeded its deadline of {limit:?}")]
        SpawnDeadlineExceeded { limit: Duration } => "runtime.spawn_deadline_exceeded", retry: true,
            "Retry the request; if it keeps failing, make the component smaller or quicker to start.";

        /// The caller's wall-clock deadline for one request
        /// (`InvokeOptions::deadline`) was already spent, or the call outlasted
        /// it.
        ///
        /// Distinct from [`Error::CpuBudgetExceeded`]: this is the caller's
        /// limit, measured in wall time, and includes host load and the guest's
        /// outbound calls; that one is the app's own sealed budget.
        #[error("invoke exceeded its deadline of {limit:?}")]
        InvokeDeadlineExceeded { limit: Duration } => "runtime.invoke_deadline_exceeded", retry: true,
            "Retry the request; if it keeps failing, make the handler finish sooner.";

        /// The guest spent its sealed `capabilities.limits.cpu_ms` budget.
        ///
        /// Enforced with epoch ticks, which advance in wall time while the guest
        /// runs, so it measures guest running time, host contention included.
        /// Not retryable: the same work hits the same limit.
        #[error("guest exceeded its CPU budget of {limit:?}")]
        CpuBudgetExceeded { limit: Duration } => "runtime.cpu_budget_exceeded", retry: false,
            "Raise `capabilities.limits.cpu_ms` in `clwd.toml` and rebuild with `clwd build`, or make the request handler do less work.";

        /// The instance is gone, so this handle can serve nothing more.
        ///
        /// This is never use-after-stop: `stop` consumes the `Handle`, so that
        /// is a compile error rather than a runtime one. A crashing guest is
        /// [`Error::Trap`], not this.
        ///
        /// Retryable means re-sending the *request*, which a host serves with a
        /// fresh instance — never invoking the same handle again, which will
        /// keep failing.
        #[error("the instance is no longer running")]
        Stopped {} => "runtime.stopped", retry: true,
            "Start the app again, and read `clwd logs` for what it printed before it stopped.";

        /// The guest tried to grow its memory past the
        /// `capabilities.limits.memory_mb` its manifest sealed.
        ///
        /// Distinct from [`Error::Trap`] on purpose: a guest that exceeded its
        /// budget and a guest with a bug are different failures, and an
        /// operator reading a log needs to tell them apart.
        #[error("guest exceeded its memory budget: requested {requested} bytes, limit {allowed}")]
        MemoryLimitExceeded { requested: u64, allowed: u64 } => "runtime.memory_limit_exceeded", retry: false,
            "Raise `capabilities.limits.memory_mb` in `clwd.toml` and rebuild with `clwd build`, or make the app allocate less.";

        /// The guest's response body passed the host's fixed size cap. Nothing
        /// in the manifest raises it, which is why it is not
        /// [`Error::MemoryLimitExceeded`]. The cap is in the message, not the
        /// hint, so the hint cannot go stale if it changes.
        #[error("response body exceeds the host's fixed limit of {allowed} bytes")]
        ResponseTooLarge { allowed: u64 } => "runtime.response_too_large", retry: false,
            "Return a smaller response body; the message gives the host's fixed limit.";

        /// The guest grew a table past the host's fixed element cap. Nothing in
        /// the manifest raises it.
        #[error("guest exceeded the host's fixed table limit: requested {requested} elements, limit {allowed}")]
        TableLimitExceeded { requested: u64, allowed: u64 } => "runtime.table_limit_exceeded", retry: false,
            "Reduce how many function references the app adds to its tables at run time; the host's limit is fixed.";

        /// The guest trapped: an unreachable, a failed assertion, a panic in
        /// guest code, or an epoch/fuel interruption.
        ///
        /// The message is preserved verbatim because it is the only diagnostic
        /// an app author gets — summarizing it here would discard the one thing
        /// that makes the failure actionable.
        #[error("guest trapped: {message}")]
        Trap { message: String } => "runtime.trap", retry: false,
            "Fix the failure the message reports in the app's code, then rebuild with `clwd build`.";

        /// A guest was refused a capability its manifest did not grant: an
        /// outbound request to an undeclared host, or an app-data operation when
        /// the manifest grants no storage.
        ///
        /// Host-side and deny-by-default. The `detail` names the specific host
        /// or namespace for the app owner's logs; it is NOT what the guest sees.
        /// A guest receives only a generic `wasi:http` `HttpRequestDenied` or
        /// app-data `invalid-key`, because an untrusted guest must not be able to
        /// probe which host or namespace it was blocked from — see the module
        /// docs in `wasm/capability.rs`.
        #[error("capability denied: {detail}")]
        CapabilityDenied { detail: String } => "runtime.capability_denied", retry: false,
            "Add the denied host to `capabilities.egress` or a namespace to `capabilities.storage_namespaces` in `clwd.toml`, then rebuild with `clwd build`.";

        /// The component imports a first-party interface at a version this
        /// runtime does not provide: almost always an app built against an
        /// older `clwd-sdk`. Refused at spawn, before the guest runs, so the
        /// author gets the interface's name rather than a trap halfway through
        /// a request.
        #[error(
            "the component imports {import}, which this runtime does not provide; \
             rebuild it against the current clwd-sdk"
        )]
        IncompatibleInterface { import: String } => "runtime.incompatible_interface", retry: false,
            "Update the app's `clwd-sdk` dependency to the current version and rebuild with `clwd build`.";

        /// Last resort only, never where a specific variant fits. A boxed
        /// opaque source in a library error type is tolerable as a genuine
        /// fallback and corrosive as a default.
        #[error("internal runtime error: {source}")]
        Internal {
            #[source]
            source: Box<dyn std::error::Error + Send + Sync>,
        } => "runtime.internal", retry: false,
            "Report this at https://github.com/clwdrun/clwd/issues with the command that produced it.";
    }
}

impl From<clwd_bundle::Error> for Error {
    fn from(source: clwd_bundle::Error) -> Self {
        Self::Bundle { source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One sample per owned variant, written by hand. `OWNED_CODES` is generated
/// from the enum, so adding a variant and forgetting it here fails
/// `owned_codes_are_exhaustive_over_variants`.
///
/// Public under `test-util` so callers' tests can check every runtime code —
/// the published error table holds its recovery column to `retryable()` with
/// it.
#[cfg(any(test, feature = "test-util"))]
pub fn samples() -> Vec<Error> {
    vec![
        Error::BundleNotFound {
            bundle: BundleRef::of_stream(b""),
        },
        Error::SpawnDeadlineExceeded {
            limit: Duration::from_secs(1),
        },
        Error::InvokeDeadlineExceeded {
            limit: Duration::from_secs(1),
        },
        Error::CpuBudgetExceeded {
            limit: Duration::from_millis(50),
        },
        Error::Stopped {},
        Error::MemoryLimitExceeded {
            requested: 2,
            allowed: 1,
        },
        Error::ResponseTooLarge { allowed: 16 },
        Error::TableLimitExceeded {
            requested: 2,
            allowed: 1,
        },
        Error::Trap {
            message: "unreachable".into(),
        },
        Error::CapabilityDenied {
            detail: "egress to \"x\" is not in the manifest allowlist".into(),
        },
        Error::IncompatibleInterface {
            import: "clwd:identity/context@0.1.0".into(),
        },
        Error::Internal {
            source: "boom".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn owned_codes_are_unique() {
        let unique: BTreeSet<&str> = OWNED_CODES.iter().copied().collect();
        assert_eq!(unique.len(), OWNED_CODES.len(), "duplicate code");
    }

    #[test]
    fn owned_codes_are_exhaustive_over_variants() {
        let produced: BTreeSet<&str> = samples().iter().map(Error::code).collect();
        let declared: BTreeSet<&str> = OWNED_CODES.iter().copied().collect();
        assert_eq!(
            produced, declared,
            "a variant is missing from `samples()` or from the enum"
        );
    }

    #[test]
    fn every_owned_code_has_a_hint() {
        let hinted: Vec<&str> = OWNED_HINTS.iter().map(|(code, _)| *code).collect();
        assert_eq!(hinted, OWNED_CODES, "OWNED_HINTS and OWNED_CODES disagree");
        for (code, hint) in OWNED_HINTS {
            assert!(!hint.trim().is_empty(), "{code} has an empty hint");
        }
    }

    /// `hint()` returns what the catalogue publishes, for every variant. A
    /// default arm placed ahead of the generated ones would shadow them and
    /// fail here.
    #[test]
    fn hint_agrees_with_the_catalogue() {
        let catalogue: std::collections::BTreeMap<&str, &str> =
            OWNED_HINTS.iter().copied().collect();
        for err in samples() {
            assert_eq!(err.hint(), catalogue[err.code()], "{}", err.code());
        }
        let bundle = Error::from(clwd_bundle::Error::ManifestMissing {});
        assert_eq!(
            bundle.hint(),
            clwd_bundle::Error::ManifestMissing {}.hint(),
            "a delegating variant forwards its source's hint"
        );
    }

    /// `retryable()` returns what the catalogue publishes, for every variant,
    /// and the delegating variant forwards its source's answer.
    #[test]
    fn retryable_agrees_with_the_catalogue() {
        let catalogue: std::collections::BTreeMap<&str, bool> =
            OWNED_RETRYABLE.iter().copied().collect();
        assert_eq!(
            catalogue.len(),
            OWNED_CODES.len(),
            "OWNED_RETRYABLE is incomplete"
        );
        for err in samples() {
            assert_eq!(err.retryable(), catalogue[err.code()], "{}", err.code());
        }
        let bundle = Error::from(clwd_bundle::Error::ManifestMissing {});
        assert_eq!(
            bundle.retryable(),
            clwd_bundle::Error::ManifestMissing {}.retryable()
        );
    }

    /// A hint never carries error data, so it never needs a placeholder.
    #[test]
    fn no_hint_is_a_format_string() {
        for (code, hint) in OWNED_HINTS {
            assert!(
                !hint.contains(['{', '}']),
                "{code}: a hint must not interpolate: {hint:?}"
            );
        }
    }

    #[test]
    fn owned_codes_are_in_the_runtime_namespace() {
        for code in OWNED_CODES {
            assert!(
                code.starts_with("runtime."),
                "{code} is not in the runtime namespace"
            );
            assert_eq!(*code, code.to_lowercase(), "{code} is not lowercase");
        }
    }
}
