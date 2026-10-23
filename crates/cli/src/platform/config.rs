//! Where a publish call learns the platform to talk to and the token to use.
//!
//! # The token, and the hazard each source carries
//!
//! A token is a bearer credential: whoever holds it can publish as its owner.
//! Two sources, in order:
//!
//! 1. `CLWD_TOKEN` — convenient for CI, and the one to reach for in a shell.
//!    The hazard is real: an environment variable is visible in `ps` and in
//!    shell history for the process's lifetime. It is the caller's to accept.
//! 2. A `credentials` file beside the local secret store, refused unless it is
//!    owner-only (`0600`) — the same posture `#27` established, checked with the
//!    same code. A world-readable token file is exactly the exposure that store
//!    exists to prevent, so a widened one is refused rather than read.
//!
//! There is deliberately no `--token` flag: a token on the command line lands in
//! `ps` and shell history for every process on the machine, and offering the
//! convenient spelling means it is the one people use — the same reasoning that
//! kept a value off `clwd secrets set`.

use std::path::{Path, PathBuf};

use super::{PlatformUrl, Token};
use crate::error::CliError;
use crate::secrets::FileStore;
use crate::secrets::value::wipe;

/// The environment variable naming the platform base URL.
const ENV_URL: &str = "CLWD_PLATFORM_URL";
/// The environment variable carrying the platform token.
const ENV_TOKEN: &str = "CLWD_TOKEN";
const ENV_ORG: &str = "CLWD_ORG";
/// The token file's name, beside the local secret store.
const CREDENTIALS_FILE: &str = "credentials";

/// Resolve the platform base URL: `--platform`, then `$CLWD_PLATFORM_URL`.
///
/// There is no built-in default while the platform is private; a run with
/// neither source is a configuration error naming both ways to fix it, rather
/// than a request sent nowhere.
pub fn platform_url(flag: Option<&str>) -> Result<PlatformUrl, CliError> {
    if let Some(raw) = flag {
        return PlatformUrl::parse(raw);
    }
    if let Some(raw) = std::env::var_os(ENV_URL) {
        let raw = raw.to_str().ok_or_else(|| CliError::Config {
            what: format!("{ENV_URL} is not valid UTF-8"),
        })?;
        return PlatformUrl::parse(raw);
    }
    Err(CliError::Config {
        what: format!("no platform URL: pass --platform <url> or set {ENV_URL}"),
    })
}

/// Resolve the org: the `--org` flag, then `$CLWD_ORG`.
///
/// A token cannot yet say which org it belongs to, so the org is named
/// explicitly. Neither source is a configuration error naming both.
pub fn org(flag: Option<&str>) -> Result<super::OrgId, CliError> {
    if let Some(raw) = flag {
        return super::OrgId::parse(raw);
    }
    if let Some(raw) = std::env::var_os(ENV_ORG) {
        let raw = raw.to_str().ok_or_else(|| CliError::Config {
            what: format!("{ENV_ORG} is not valid UTF-8"),
        })?;
        return super::OrgId::parse(raw.trim());
    }
    Err(CliError::Config {
        what: format!("no org: pass --org <id> or set {ENV_ORG}"),
    })
}

/// Resolve the platform token: `$CLWD_TOKEN`, then the `0600` credentials file.
pub fn token() -> Result<Token, CliError> {
    if let Some(raw) = std::env::var_os(ENV_TOKEN) {
        let text = raw.to_str().ok_or_else(|| CliError::Config {
            what: format!("{ENV_TOKEN} is not valid UTF-8"),
        })?;
        return Token::parse(text.trim());
    }

    let path = credentials_path()?;
    if let Some(token) = token_from_file(&path)? {
        return Ok(token);
    }
    Err(CliError::Config {
        what: format!(
            "no platform token: set {ENV_TOKEN}, or write {} with mode 600",
            path.display()
        ),
    })
}

/// Where [`token`] looks for the credentials file: beside the secret store.
fn credentials_path() -> Result<PathBuf, CliError> {
    Ok(FileStore::default_path()?.with_file_name(CREDENTIALS_FILE))
}

/// Read a token from `path`, or `None` if the file does not exist.
///
/// A file that exists but is not owner-only is refused, not read — reusing the
/// secret store's own check so the two cannot drift apart.
fn token_from_file(path: &Path) -> Result<Option<Token>, CliError> {
    let mut raw = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(CliError::Config {
                what: format!("{} could not be read: {e}", path.display()),
            });
        }
    };

    if let Err(e) = crate::secrets::store::verify_private(path) {
        wipe(&mut raw);
        return Err(e);
    }

    let token = match std::str::from_utf8(&raw) {
        Ok(text) => Token::parse(text.trim()).map(Some),
        Err(_) => Err(CliError::Config {
            what: format!("{} is not valid UTF-8", path.display()),
        }),
    };
    wipe(&mut raw);
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "clwd_tok_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "clwd-creds-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn path(&self) -> PathBuf {
            self.0.join(CREDENTIALS_FILE)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn write_mode(path: &Path, contents: &str, mode: u32) {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(path)
            .expect("open");
        f.write_all(contents.as_bytes()).expect("write");
    }

    #[test]
    fn an_absent_file_is_none_not_an_error() {
        let s = Scratch::new("absent");
        assert!(token_from_file(&s.path()).expect("read").is_none());
    }

    /// Invariant: CLI-1.
    #[cfg(unix)]
    #[test]
    fn an_owner_only_file_yields_the_token() {
        let s = Scratch::new("valid");
        write_mode(&s.path(), &format!("{TOKEN}\n"), 0o600);
        let token = token_from_file(&s.path())
            .expect("read")
            .expect("a token is present");
        assert_eq!(
            token.as_bearer(),
            TOKEN,
            "the trailing newline must be trimmed"
        );
    }

    /// Invariant: CLI-1.
    #[cfg(unix)]
    #[test]
    fn a_world_readable_token_file_is_refused() {
        let s = Scratch::new("widened");
        write_mode(&s.path(), TOKEN, 0o644);
        let err = token_from_file(&s.path()).expect_err("a widened file must be refused");
        assert_eq!(err.code(), "cli.config");
    }

    #[cfg(unix)]
    #[test]
    fn a_malformed_token_in_the_file_is_rejected() {
        let s = Scratch::new("malformed");
        write_mode(&s.path(), "not-a-token", 0o600);
        let err = token_from_file(&s.path()).expect_err("a bad token must be rejected");
        assert_eq!(err.code(), "cli.invalid_argument");
    }

    #[test]
    fn a_platform_url_flag_is_taken_before_the_environment() {
        let url = platform_url(Some("https://flag.example")).expect("valid");
        assert_eq!(url.to_string(), "https://flag.example/");
    }
}
