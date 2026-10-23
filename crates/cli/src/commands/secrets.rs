//! `clwd secrets` — local secret management for development.
//!
//! # This surface deliberately differs from the guest's
//!
//! `clwd:secrets/broker` makes "no such secret" and "not yours"
//! indistinguishable, and that is load-bearing: a guest must not be able to
//! probe a namespace it was not granted, so `get` returns `option` with no
//! error variant to branch on.
//!
//! **This command does the opposite, on purpose.** It is operator-facing, and
//! the operator owns the store. Telling them a name is not there leaks nothing
//! to anyone — there is no untrusted party in the room — and it is the only
//! useful answer to `clwd secrets get`. Refusing to say would be an error in
//! the other direction: security theatre that costs a real person their
//! afternoon.
//!
//! The rule being preserved is narrower than "never distinguish". It is that
//! **nothing on the injection path gains a distinction**: the guest still
//! cannot tell the two apart, because `resolve` returns names it holds and says
//! nothing about names it does not. Someone will eventually read this file and
//! flag it as inconsistent with the WIT; it is not, and this paragraph is why.

use std::io::Read;
use std::path::Path;

use crate::build::Source;
use crate::error::CliError;
use crate::secrets::{AppName, FileStore, SecretName, SecretValue};

/// The most a value is allowed to be. Generous for a key or certificate, and
/// small enough that a mistyped `<` redirecting a disk image is refused rather
/// than read into memory.
const MAX_VALUE: usize = 1024 * 1024;

/// What `set` and `rm` report. Never a value.
#[derive(Debug, serde::Serialize)]
#[non_exhaustive]
pub struct Changed {
    pub app: String,
    pub name: String,
    /// How many bytes were stored. Safe to say, and the one fact that confirms
    /// the right thing arrived — a zero here means an empty pipe.
    pub bytes: Option<usize>,
    pub removed: bool,
}

/// What `list` reports: **names only**.
///
/// There is no value field, and there is no `#[serde(flatten)]` of anything
/// that has one. `--json` is a separate rendering path from the human one and
/// is the easy place for a value to escape, so the type that path serializes
/// simply cannot carry one.
#[derive(Debug, serde::Serialize)]
#[non_exhaustive]
pub struct Listing {
    pub app: String,
    pub names: Vec<String>,
}

/// The app whose namespace this project's secrets live in.
///
/// Read from `[app] name` in `clwd.toml`, and required. There is deliberately
/// no fallback to the directory name: two projects sharing a namespace should
/// be something an author chose, not a consequence of where a repository
/// happened to be cloned.
fn app_of(project: &Path) -> Result<AppName, CliError> {
    let path = project.join(crate::build::manifest::FILENAME);
    let text = std::fs::read_to_string(&path).map_err(|e| CliError::NotFound {
        what: format!(
            "{} could not be read ({e}). `clwd secrets` needs it to know which app \
             these secrets belong to",
            path.display()
        ),
    })?;
    let source = Source::parse(&text)?;
    let name = source.app_name().ok_or_else(|| CliError::ManifestInvalid {
        what: format!(
            "{} has no `name` under [app]. Secrets are scoped to an app name, so add:\n\n\
             [app]\nname = \"your-app\"\n",
            path.display()
        ),
    })?;
    AppName::new(name)
}

/// Store a secret, reading its value from `input`.
pub fn set(project: &Path, name: &str, input: &mut dyn Read) -> Result<Changed, CliError> {
    let app = app_of(project)?;
    let name = SecretName::new(name)?;

    // Bounded, so a mistyped redirect is refused rather than read whole.
    let mut buffer = Vec::new();
    input
        .take(MAX_VALUE as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|e| CliError::InvalidArgument {
            what: format!("the value could not be read from stdin: {e}"),
        })?;

    if buffer.len() > MAX_VALUE {
        crate::secrets::value::wipe(&mut buffer);
        return Err(CliError::InvalidArgument {
            what: format!("the value is larger than the {MAX_VALUE} byte limit"),
        });
    }
    if buffer.is_empty() {
        return Err(CliError::InvalidArgument {
            what: "the value is empty — pipe it in, as `clwd secrets set NAME < file`".to_owned(),
        });
    }

    let bytes = buffer.len();
    let value = SecretValue::new(buffer);
    FileStore::open()?.set(&app, &name, &value)?;

    Ok(Changed {
        app: app.as_str().to_owned(),
        name: name.as_str().to_owned(),
        bytes: Some(bytes),
        removed: false,
    })
}

/// The raw bytes of a secret, for the caller to write wherever it likes.
///
/// Returns the value rather than printing it: what reaches stdout is the
/// binary's decision, and a library function that wrote to a stream could not
/// be used by #32 at all.
pub fn get(project: &Path, name: &str) -> Result<SecretValue, CliError> {
    let app = app_of(project)?;
    let name = SecretName::new(name)?;
    FileStore::open()?
        .get(&app, &name)?
        .ok_or_else(|| CliError::NotFound {
            what: format!("{app} has no secret named {name}"),
        })
}

/// The names of this app's secrets.
pub fn list(project: &Path) -> Result<Listing, CliError> {
    let app = app_of(project)?;
    let names = FileStore::open()?
        .list(&app)?
        .iter()
        .map(|n| n.as_str().to_owned())
        .collect();
    Ok(Listing {
        app: app.as_str().to_owned(),
        names,
    })
}

/// Remove a secret. Removing what is not there succeeds.
pub fn remove(project: &Path, name: &str) -> Result<Changed, CliError> {
    let app = app_of(project)?;
    let name = SecretName::new(name)?;
    let removed = FileStore::open()?.remove(&app, &name)?;
    Ok(Changed {
        app: app.as_str().to_owned(),
        name: name.as_str().to_owned(),
        bytes: None,
        removed,
    })
}

impl std::fmt::Display for Changed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.removed {
            write!(f, "removed {} from {}", self.name, self.app)
        } else if let Some(bytes) = self.bytes {
            write!(f, "stored {} for {} ({bytes} bytes)", self.name, self.app)
        } else {
            write!(f, "{} had no secret named {}", self.app, self.name)
        }
    }
}

impl std::fmt::Display for Listing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.names.is_empty() {
            return write!(f, "{} has no secrets", self.app);
        }
        for name in &self.names {
            writeln!(f, "{name}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Neither renderer may carry a value. `Listing` has no field for one, and
    /// this asserts the serialized shape rather than trusting the struct — a
    /// later `#[serde(flatten)]` would be caught here.
    ///
    /// Invariant: CLI-6.
    #[test]
    fn the_json_listing_carries_names_only() {
        let listing = Listing {
            app: "dashboard".to_owned(),
            names: vec!["token".to_owned()],
        };
        let json = serde_json::to_value(&listing).expect("serializes");
        let keys: Vec<&str> = json
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["app", "names"], "an unexpected field appeared");
    }

    #[test]
    fn the_human_listing_prints_names_only() {
        let listing = Listing {
            app: "dashboard".to_owned(),
            names: vec!["api-token".to_owned(), "signing-key".to_owned()],
        };
        let rendered = listing.to_string();
        assert!(rendered.contains("api-token"));
        assert!(rendered.contains("signing-key"));
        assert!(!rendered.contains("dashboard"), "got: {rendered}");
    }

    #[test]
    fn an_empty_listing_says_so_rather_than_printing_nothing() {
        let listing = Listing {
            app: "dashboard".to_owned(),
            names: Vec::new(),
        };
        assert!(listing.to_string().contains("no secrets"));
    }

    /// `set` reports the byte count, which confirms the right thing arrived
    /// without disclosing what it was.
    ///
    /// Invariant: CLI-6.
    #[test]
    fn a_change_reports_a_size_but_never_a_value() {
        let c = Changed {
            app: "a".to_owned(),
            name: "n".to_owned(),
            bytes: Some(7),
            removed: false,
        };
        let json = serde_json::to_string(&c).expect("serializes");
        assert!(json.contains("\"bytes\":7"));
        let keys = ["app", "name", "bytes", "removed"];
        for k in keys {
            assert!(json.contains(k), "{k} missing from {json}");
        }
    }
}
