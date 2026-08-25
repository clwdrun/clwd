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

/// Declares `Error`, its `code()`, and the list of codes this crate owns.
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
                $ovariant:ident { $($ofield:tt)* } => $ocode:literal;
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
        }

        /// Every code this crate owns. Delegated codes belong to the crate that
        /// defines them and are catalogued there.
        #[cfg(test)]
        pub(crate) const OWNED_CODES: &[&str] = &[$($ocode),*];
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
        BundleNotFound { bundle: BundleRef } => "runtime.bundle_not_found";

        /// A deadline expired. `phase` says which budget: cold start and
        /// invocation are separate, and #17 must be able to blow one without
        /// blaming the other.
        #[error("{phase} exceeded its deadline of {limit:?}")]
        DeadlineExceeded { phase: Phase, limit: Duration } => "runtime.deadline_exceeded";

        /// The instance is gone — it crashed, was reclaimed, or its guest
        /// exited.
        ///
        /// This is never use-after-stop: `stop` consumes the `Handle`, so that
        /// is a compile error rather than a runtime one.
        #[error("the instance is no longer running")]
        Stopped {} => "runtime.stopped";

        /// The guest asked for more of a resource than its manifest sealed.
        ///
        /// Distinct from [`Error::Trap`] on purpose: a guest that exceeded its
        /// budget and a guest with a bug are different failures, and an
        /// operator reading a log needs to tell them apart.
        #[error("guest exceeded its {limit} budget: requested {requested}, limit {allowed}")]
        ResourceExhausted {
            limit: &'static str,
            requested: u64,
            allowed: u64,
        } => "runtime.resource_exhausted";

        /// The guest trapped: an unreachable, a failed assertion, a panic in
        /// guest code, or an epoch/fuel interruption.
        ///
        /// The message is preserved verbatim because it is the only diagnostic
        /// an app author gets — summarizing it here would discard the one thing
        /// that makes the failure actionable.
        #[error("guest trapped: {message}")]
        Trap { message: String } => "runtime.trap";

        /// Last resort only, never where a specific variant fits. A boxed
        /// opaque source in a library error type is tolerable as a genuine
        /// fallback and corrosive as a default.
        #[error("internal runtime error: {source}")]
        Internal {
            #[source]
            source: Box<dyn std::error::Error + Send + Sync>,
        } => "runtime.internal";
    }
}

/// Which budget a deadline belonged to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Spawn,
    Invoke,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Spawn => "spawn",
            Self::Invoke => "invoke",
        })
    }
}

impl From<clwd_bundle::Error> for Error {
    fn from(source: clwd_bundle::Error) -> Self {
        Self::Bundle { source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// One sample per owned variant, written by hand. `OWNED_CODES` is
    /// generated from the enum, so adding a variant and forgetting it here
    /// fails the exhaustiveness test below.
    fn owned_samples() -> Vec<Error> {
        vec![
            Error::BundleNotFound {
                bundle: BundleRef::of_stream(b""),
            },
            Error::DeadlineExceeded {
                phase: Phase::Spawn,
                limit: Duration::from_secs(1),
            },
            Error::Stopped {},
            Error::Trap {
                message: "unreachable".into(),
            },
            Error::ResourceExhausted {
                limit: "memory",
                requested: 2,
                allowed: 1,
            },
            Error::Internal {
                source: "boom".into(),
            },
        ]
    }

    #[test]
    fn owned_codes_are_unique() {
        let unique: BTreeSet<&str> = OWNED_CODES.iter().copied().collect();
        assert_eq!(unique.len(), OWNED_CODES.len(), "duplicate code");
    }

    #[test]
    fn owned_codes_are_exhaustive_over_variants() {
        let produced: BTreeSet<&str> = owned_samples().iter().map(Error::code).collect();
        let declared: BTreeSet<&str> = OWNED_CODES.iter().copied().collect();
        assert_eq!(
            produced, declared,
            "a variant is missing from `owned_samples()` or from the enum"
        );
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
