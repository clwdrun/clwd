//! The error type at the library boundary.
//!
//! Every variant carries a stable, namespaced code. Those codes are a
//! compatibility surface: `--json` emits them, and #31 pins them. Renaming one
//! is a breaking change, so they are declared in exactly one place — the macro
//! invocation below — and a test holds the declaration to the variant list.

/// Declares `CliError` and, from the same input, the list of every code it can
/// produce. A variant cannot be added without a code, which is what makes the
/// exhaustiveness test in this module able to tell the truth.
macro_rules! define_cli_error {
    (
        $(
            $(#[$attr:meta])*
            $variant:ident { $($field:tt)* } => $code:literal;
        )*
    ) => {
        #[derive(Debug, thiserror::Error)]
        #[non_exhaustive]
        pub enum CliError {
            $( $(#[$attr])* $variant { $($field)* }, )*
        }

        impl CliError {
            /// The stable code for this error, as emitted by `--json`.
            pub fn code(&self) -> &'static str {
                match self {
                    $( Self::$variant { .. } => $code, )*
                }
            }
        }

        #[cfg(test)]
        const DECLARED_CODES: &[&str] = &[$($code),*];
    };
}

define_cli_error! {
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

    /// Last resort only. Never returned where a specific variant fits: an
    /// `anyhow` source inside a library error type is tolerable as a genuine
    /// fallback and corrosive as a default.
    #[error("internal error: {source}")]
    Internal {
        #[source]
        source: anyhow::Error,
    } => "cli.internal";
}

impl CliError {
    /// A short, actionable next step, when there is one worth saying.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::InvalidArgument { .. } => Some("run `clwd <command> --help` for accepted values"),
            Self::NotFound { .. } => None,
            Self::NotImplemented { .. } => {
                Some("track progress at https://github.com/clwdrun/clwd/issues")
            }
            Self::Config { .. } => None,
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
            Self::InvalidArgument { .. }
            | Self::NotFound { .. }
            | Self::NotImplemented { .. }
            | Self::Config { .. }
            | Self::Internal { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// One sample per variant, written by hand. `DECLARED_CODES` is generated
    /// from the enum definition, so the equality assertion below fails the
    /// moment a variant is added to the enum and forgotten here.
    fn samples() -> Vec<CliError> {
        vec![
            CliError::InvalidArgument { what: "x".into() },
            CliError::NotFound { what: "x".into() },
            CliError::NotImplemented { command: "x" },
            CliError::Config { what: "x".into() },
            CliError::Internal {
                source: anyhow::anyhow!("x"),
            },
        ]
    }

    #[test]
    fn codes_are_unique() {
        let unique: BTreeSet<&str> = DECLARED_CODES.iter().copied().collect();
        assert_eq!(
            unique.len(),
            DECLARED_CODES.len(),
            "duplicate code in {DECLARED_CODES:?}"
        );
    }

    #[test]
    fn codes_are_exhaustive_over_variants() {
        let produced: BTreeSet<&str> = samples().iter().map(CliError::code).collect();
        let declared: BTreeSet<&str> = DECLARED_CODES.iter().copied().collect();
        assert_eq!(
            produced, declared,
            "a variant is missing from `samples()` or from the enum"
        );
    }

    #[test]
    fn codes_are_namespaced_and_lowercase() {
        for code in DECLARED_CODES {
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
