//! The seam to a clwd platform: the newtypes a publish call needs, the error a
//! platform decision arrives as, and the HTTP call itself.
//!
//! # Why this is not behind `run`
//!
//! `clwd run` carries a sandbox and hides behind the `run` feature so a runtime
//! backend cannot leak into a build that only wants to publish. Publishing has
//! no sandbox: it reads a `.clwd` file and makes one HTTP request. So the client
//! is unconditional, and `reqwest` — already declared for exactly this — is a
//! core CLI dependency rather than a `run` one. The `cli-deps` gate still holds:
//! `reqwest` with the `rustls` backend drags in no wasmtime.
//!
//! # Three identifiers that must not substitute for one another
//!
//! A platform URL, an app id and a token are all strings and mean entirely
//! different things. They are separate types so the compiler refuses the mix-up,
//! for the same reason [`crate::secrets`] keeps app and secret names apart.

use std::fmt;

use crate::error::CliError;
use crate::secrets::value::SecretValue;

pub mod client;
pub mod config;

pub use client::{
    AppSummary, Certificate, CreatedApp, LogPage, LogQuery, Published, app_by_label, create_app,
    publish, read_logs,
};

/// A platform's base URL, validated to be http(s).
///
/// Holds a parsed [`reqwest::Url`] so the route can be built without
/// string-concatenating a path — see [`PlatformUrl::versions_endpoint`].
#[derive(Debug, Clone)]
pub struct PlatformUrl(reqwest::Url);

impl PlatformUrl {
    /// Parse and validate a base URL.
    ///
    /// The scheme must be `http` or `https`; anything else is a configuration
    /// error rather than a request that fails later with a worse message.
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        let url = reqwest::Url::parse(raw).map_err(|e| CliError::Config {
            what: format!("{raw:?} is not a valid platform URL: {e}"),
        })?;
        match url.scheme() {
            "http" | "https" => Ok(Self(url)),
            other => Err(CliError::Config {
                what: format!("the platform URL scheme {other:?} is not http or https"),
            }),
        }
    }

    /// The versions endpoint for `app`: `{base}/v1/apps/{app_id}/versions`.
    ///
    /// Built by extending path segments rather than formatting a string, so a
    /// base URL that already carries a path (a reverse proxy prefix, say) is
    /// preserved rather than clobbered.
    pub fn versions_endpoint(&self, app: &AppId) -> Result<reqwest::Url, CliError> {
        self.endpoint(&["v1", "apps", app.as_str(), "versions"])
    }

    /// `POST {base}/v1/orgs/{org}/apps` — create an app in `org`.
    pub fn apps_endpoint(&self, org: &OrgId) -> Result<reqwest::Url, CliError> {
        self.endpoint(&["v1", "orgs", org.as_str(), "apps"])
    }

    /// `GET {base}/v1/orgs/{org}/apps/{host_label}` — one app, by its label.
    pub fn app_by_label_endpoint(
        &self,
        org: &OrgId,
        host_label: &str,
    ) -> Result<reqwest::Url, CliError> {
        self.endpoint(&["v1", "orgs", org.as_str(), "apps", host_label])
    }

    /// `GET {base}/v1/apps/{app_id}/logs` — an app's log.
    pub fn logs_endpoint(&self, app: &AppId) -> Result<reqwest::Url, CliError> {
        self.endpoint(&["v1", "apps", app.as_str(), "logs"])
    }

    fn endpoint(&self, path: &[&str]) -> Result<reqwest::Url, CliError> {
        let mut url = self.0.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|()| CliError::Config {
                what: format!("{} cannot be a base URL for the platform", self.0),
            })?;
            // `pop_if_empty` drops the trailing empty segment a bare host carries
            // (`http://host` has path `/`), so the join does not leave `//`.
            segments.pop_if_empty().extend(path);
        }
        Ok(url)
    }
}

impl fmt::Display for PlatformUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// An app's platform identity: a UUID.
///
/// **Not the `[app] name` from `clwd.toml`.** That name scopes local secrets in
/// [`crate::secrets`] and is a different identifier entirely. The platform mints
/// the id; the CLI only carries it. Validated to be UUID-shaped locally so a
/// typo is a `cli.invalid_argument` here rather than a round trip that 404s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppId(String);

impl AppId {
    /// Validate and wrap. A non-UUID is rejected locally.
    ///
    /// The check is the shape — 8-4-4-4-12 hex — not a full UUID parse: the
    /// platform is the authority on whether an id names a live app, and the
    /// only job here is to refuse an argument that plainly cannot.
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        if is_uuid_shaped(raw) {
            Ok(Self(raw.to_ascii_lowercase()))
        } else {
            Err(CliError::InvalidArgument {
                what: format!(
                    "{raw:?} is not an app id; it must be a UUID like \
                     018f5b2a-0000-7000-8000-000000000000"
                ),
            })
        }
    }

    /// The id as a URL path segment.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An org's platform identity: a UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgId(String);

impl OrgId {
    /// Validate and wrap. As with [`AppId`], only the shape is checked here.
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        if is_uuid_shaped(raw) {
            Ok(Self(raw.to_ascii_lowercase()))
        } else {
            Err(CliError::InvalidArgument {
                what: format!(
                    "{raw:?} is not an org id; it must be a UUID like \
                     018f5b2a-0000-7000-8000-000000000000"
                ),
            })
        }
    }

    /// The id as a URL path segment.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `s` is shaped like a canonical UUID: 36 chars, hyphens at 8/13/18/23,
/// hex everywhere else. Case-insensitive.
fn is_uuid_shaped(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => *c == b'-',
        _ => c.is_ascii_hexdigit(),
    })
}

/// The prefix every platform token carries.
const TOKEN_PREFIX: &str = "clwd_tok_";
/// The hex body after [`TOKEN_PREFIX`].
const TOKEN_BODY_LEN: usize = 64;

/// A platform bearer token.
///
/// Wraps a [`SecretValue`], so it inherits the redaction that type enforces: no
/// `Display`, and a `Debug` that never prints the bytes. A token cannot reach a
/// log line, an error message or `--json` output through a formatter, which is
/// the leak a CLI actually suffers.
pub struct Token(SecretValue);

impl Token {
    /// Validate and wrap. The shape is `clwd_tok_` followed by 64 hex characters.
    ///
    /// The rejected value is never echoed in the error: a malformed token is
    /// still a token, and putting it in a message would defeat the point.
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        if is_valid_token(raw) {
            Ok(Self(SecretValue::new(raw.as_bytes().to_vec())))
        } else {
            Err(CliError::InvalidArgument {
                what: format!(
                    "a platform token must be {TOKEN_PREFIX} followed by \
                     {TOKEN_BODY_LEN} hex characters"
                ),
            })
        }
    }

    /// The token as an `Authorization: Bearer` value.
    ///
    /// `pub(crate)` and named so the one place a token is meant to leave the
    /// type — the request header — is greppable, mirroring [`SecretValue::expose`].
    /// The bytes are ASCII by construction; the fallback is unreachable and only
    /// there to keep the function panic-free.
    pub(crate) fn as_bearer(&self) -> &str {
        std::str::from_utf8(self.0.expose()).unwrap_or_default()
    }
}

/// Whether `s` is a well-formed platform token: `clwd_tok_` + 64 hex.
fn is_valid_token(s: &str) -> bool {
    let Some(body) = s.strip_prefix(TOKEN_PREFIX) else {
        return false;
    };
    body.len() == TOKEN_BODY_LEN && body.bytes().all(|c| c.is_ascii_hexdigit())
}

impl fmt::Debug for Token {
    /// Never the value. A token is a credential; its length is fixed and
    /// uninteresting, so unlike [`SecretValue`] there is nothing to disclose.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(redacted)")
    }
}

/// A failure a platform decided, carrying the shared error envelope verbatim.
///
/// `platform#53` guarantees every 4xx/5xx response is
/// `{code, message, hint, retryable}`. The CLI surfaces that `code` unchanged —
/// wrapping `identity.forbidden` in `cli.internal` would tell a caller only that
/// something went wrong, when the platform already said exactly what. This type
/// is the delegating source behind [`CliError::Platform`].
#[derive(Debug)]
pub struct PlatformError {
    code: String,
    message: String,
    hint: Option<String>,
    retryable: bool,
}

impl PlatformError {
    /// Build from the envelope's four fields.
    #[must_use]
    pub(crate) fn new(
        code: String,
        message: String,
        hint: Option<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code,
            message,
            hint,
            retryable,
        }
    }

    /// The platform's code, surfaced unchanged.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The platform's hint, if it gave one.
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Whether the platform said retrying could succeed.
    #[must_use]
    pub fn retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PlatformError {}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_TOKEN: &str =
        "clwd_tok_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const VALID_APP: &str = "018f5b2a-0000-7000-8000-000000000000";

    #[test]
    fn an_app_id_must_be_uuid_shaped() {
        assert!(AppId::parse(VALID_APP).is_ok());
        for bad in [
            "",
            "not-a-uuid",
            "dashboard",
            &"0".repeat(36),
            "018f5b2a00007000",
        ] {
            let err = AppId::parse(bad).expect_err(&format!("{bad:?} is not a UUID"));
            assert_eq!(err.code(), "cli.invalid_argument");
        }
    }

    #[test]
    fn an_app_id_is_lowercased_to_canonical_form() {
        let upper = "018F5B2A-0000-7000-8000-00000000000A";
        assert_eq!(
            AppId::parse(upper).expect("valid uuid shape").as_str(),
            upper.to_ascii_lowercase()
        );
    }

    #[test]
    fn a_token_must_match_the_shape() {
        assert!(Token::parse(VALID_TOKEN).is_ok());
        for bad in [
            "",
            "clwd_tok_",
            "clwd_tok_short",
            "wrong_prefix_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "clwd_tok_zzzz456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ] {
            let err = Token::parse(bad).expect_err(&format!("{bad:?} is not a token"));
            assert_eq!(err.code(), "cli.invalid_argument");
        }
    }

    /// A malformed token must not be echoed back in the error that rejects it.
    #[test]
    fn a_rejected_token_is_not_repeated_in_the_error() {
        let secret = "clwd_tok_this_is_the_secret_that_must_not_leak_even_when_rejected";
        let err = Token::parse(secret).expect_err("too short to be valid");
        assert!(
            !err.to_string().contains("secret"),
            "the rejected token leaked into the error: {err}"
        );
    }

    /// The whole point of building on `SecretValue`: a token cannot be formatted.
    #[test]
    fn a_token_redacts_itself_in_debug() {
        let token = Token::parse(VALID_TOKEN).expect("valid");
        let rendered = format!("{token:?}");
        assert!(
            !rendered.contains("0123456789"),
            "the token leaked through Debug: {rendered}"
        );
        assert!(rendered.contains("redacted"), "got: {rendered}");
    }

    #[test]
    fn the_versions_endpoint_is_built_from_the_base() {
        let url = PlatformUrl::parse("https://api.clwd.example").expect("valid");
        let app = AppId::parse(VALID_APP).expect("valid");
        let endpoint = url.versions_endpoint(&app).expect("buildable");
        assert_eq!(
            endpoint.as_str(),
            format!("https://api.clwd.example/v1/apps/{VALID_APP}/versions")
        );
    }

    /// A base URL that already carries a path prefix keeps it.
    #[test]
    fn a_base_path_prefix_is_preserved() {
        let url = PlatformUrl::parse("https://gw.example/clwd/").expect("valid");
        let app = AppId::parse(VALID_APP).expect("valid");
        let endpoint = url.versions_endpoint(&app).expect("buildable");
        assert_eq!(
            endpoint.as_str(),
            format!("https://gw.example/clwd/v1/apps/{VALID_APP}/versions")
        );
    }

    #[test]
    fn a_non_http_scheme_is_refused() {
        let err = PlatformUrl::parse("ftp://example.com").expect_err("not http");
        assert_eq!(err.code(), "cli.config");
    }
}
