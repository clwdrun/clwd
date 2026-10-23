//! The workspace root: the one directory tree the server's tools act in.
//!
//! A path from the client is relative, joined to the root, and resolved by the
//! operating system — `..` and every symlink along the way — before it is
//! compared. A path is inside the root when what it resolves to is.
//!
//! ponytail: resolve-then-use has a gap: a symlink swapped in between the check
//! and the use is followed. Acceptable for a single-user local server, where
//! whoever can swap it can already write the files; `openat2` with
//! `RESOLVE_BENEATH` closes it if this ever serves more than one user.

use std::path::{Component, Path, PathBuf};

use crate::error::CliError;

#[derive(Debug, Clone)]
pub struct Root(PathBuf);

impl Root {
    /// `dir`, resolved. It must exist and be a directory.
    pub fn new(dir: &Path) -> Result<Self, CliError> {
        let resolved = dir.canonicalize().map_err(|e| CliError::Config {
            what: format!(
                "the workspace root {} could not be resolved: {e}",
                dir.display()
            ),
        })?;
        if !resolved.is_dir() {
            return Err(CliError::Config {
                what: format!("the workspace root {} is not a directory", dir.display()),
            });
        }
        Ok(Self(resolved))
    }

    /// The existing directory `raw` names, relative to the root.
    pub fn dir(&self, raw: &str) -> Result<PathBuf, CliError> {
        let joined = self.join(raw)?;
        let resolved = joined.canonicalize().map_err(|e| CliError::NotFound {
            what: format!("{raw:?} in the workspace root: {e}"),
        })?;
        self.contains(raw, &resolved)?;
        if !resolved.is_dir() {
            return Err(CliError::InvalidArgument {
                what: format!("{raw:?} is not a directory"),
            });
        }
        Ok(resolved)
    }

    /// `name` inside the directory `parent`, which [`Root::dir`] resolved.
    /// When it exists already — as a symlink, say — what it resolves to must
    /// be inside the root too.
    pub fn child(&self, parent: &Path, name: &str) -> Result<PathBuf, CliError> {
        let path = parent.join(name);
        if path.symlink_metadata().is_ok() {
            let resolved = path.canonicalize().map_err(|e| CliError::InvalidArgument {
                what: format!("{name:?} exists and could not be resolved: {e}"),
            })?;
            self.contains(name, &resolved)?;
        }
        Ok(path)
    }

    /// `path` as the client should see it: relative to the root when inside it.
    #[must_use]
    pub fn show(&self, path: &Path) -> String {
        path.strip_prefix(&self.0).map_or_else(
            |_| path.display().to_string(),
            |p| {
                if p.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    p.display().to_string()
                }
            },
        )
    }

    fn join(&self, raw: &str) -> Result<PathBuf, CliError> {
        let path = Path::new(raw);
        if path
            .components()
            .any(|c| matches!(c, Component::RootDir | Component::Prefix(_)))
        {
            return Err(CliError::InvalidArgument {
                what: format!(
                    "{raw:?} is an absolute path; tools take a path relative to the workspace root"
                ),
            });
        }
        Ok(self.0.join(path))
    }

    fn contains(&self, raw: &str, resolved: &Path) -> Result<(), CliError> {
        if resolved.starts_with(&self.0) {
            Ok(())
        } else {
            Err(CliError::InvalidArgument {
                what: format!(
                    "{raw:?} resolves outside the workspace root; tools act only within it"
                ),
            })
        }
    }
}
