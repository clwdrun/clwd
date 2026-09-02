//! The local development secret store: one JSON file, owner-readable only.
//!
//! # What this is, and what it is not
//!
//! **This is for a development laptop. It is not for anything shared.** The
//! file is protected by filesystem permissions and nothing else: anyone who can
//! read it — another account with the same uid, a backup, a synced folder — has
//! every secret in it in plaintext. The production answer is `platform#22`,
//! which is a different store with a different threat model, reached through
//! the same `SecretStore` trait.
//!
//! The threat model here is exactly one thing: **other users on the same
//! machine**. `0600` on the file and `0700` on its directory is what stops
//! them, and it is what every comparable developer tool does — `~/.aws/
//! credentials`, `~/.npmrc`, `~/.docker/config.json`. It does not protect
//! against someone who already has the account.
//!
//! # Why a plain file rather than the OS keyring
//!
//! `keyring` stays declared in `Cargo.toml` as a future opt-in backend behind a
//! feature — it is not dead weight and should not be removed. It was measured
//! and not chosen: it adds 56 crates to this workspace including a second async
//! executor stack beside the tokio the CLI already runs, and on a headless
//! Linux box there is no D-Bus session to talk to. Its `keyring-core` mock
//! removes the need to skip tests, but replaces that with a subtler problem —
//! green everywhere, with the real backend exercised nowhere. A file store has
//! no such gap: the tests below run the real implementation on every platform.
//!
//! Encrypting the file was considered and rejected on key management rather
//! than on cost. The key has to live somewhere: a passphrase means a prompt on
//! every `clwd run`, an identity file beside the ciphertext is decoration, and
//! putting the key in the OS keyring is circular.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use super::value::{AppName, SecretName, SecretValue, wipe};
use crate::error::CliError;

/// `rw-------`: the owner, and nobody else.
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;
/// `rwx------`. A directory needs execute to be traversed by its owner.
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

/// The on-disk shape: app name to secret name to base64 value.
///
/// `BTreeMap` rather than `HashMap` so the file is byte-stable across writes.
/// A store that reorders itself on every save produces noise in any diff and
/// makes "did this change?" unanswerable by looking.
type Disk = BTreeMap<String, BTreeMap<String, String>>;

/// Secrets for local development, in one file.
#[derive(Debug)]
#[non_exhaustive]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// Open the store at the default location.
    ///
    /// `$XDG_CONFIG_HOME/clwd/secrets.json`, falling back to
    /// `$HOME/.config/clwd/secrets.json`. Resolved from the environment rather
    /// than through a crate: it is two variables and one fallback, and a
    /// dependency for that would be more code to audit than it replaces.
    pub fn open() -> Result<Self, CliError> {
        Ok(Self::at(Self::default_path()?))
    }

    /// Open the store at an explicit path.
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where [`Self::open`] would look.
    pub fn default_path() -> Result<PathBuf, CliError> {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(xdg).join("clwd").join("secrets.json"));
        }
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| CliError::Config {
                what: "neither XDG_CONFIG_HOME nor HOME is set, so there is nowhere to \
                       keep local secrets"
                    .to_owned(),
            })?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join("clwd")
            .join("secrets.json"))
    }

    /// The file backing this store.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every secret name held for `app`, sorted. **Names only.**
    pub fn list(&self, app: &AppName) -> Result<Vec<SecretName>, CliError> {
        let disk = self.read()?;
        disk.get(app.as_str())
            .map(|entries| entries.keys().map(|k| SecretName::new(k)).collect())
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    /// The value of `name` for `app`, if it is held.
    pub fn get(&self, app: &AppName, name: &SecretName) -> Result<Option<SecretValue>, CliError> {
        let disk = self.read()?;
        let Some(encoded) = disk.get(app.as_str()).and_then(|e| e.get(name.as_str())) else {
            return Ok(None);
        };
        decode(encoded).map(Some)
    }

    /// Store `value` under `name` for `app`, replacing any current value.
    pub fn set(
        &self,
        app: &AppName,
        name: &SecretName,
        value: &SecretValue,
    ) -> Result<(), CliError> {
        let mut disk = self.read()?;
        disk.entry(app.as_str().to_owned())
            .or_default()
            .insert(name.as_str().to_owned(), encode(value));
        self.write(&disk)
    }

    /// Remove `name` from `app`. Returns whether anything was removed.
    ///
    /// Removing what is not there succeeds, matching `app-data`'s delete: an
    /// idempotent remove is one a script can call without checking first.
    pub fn remove(&self, app: &AppName, name: &SecretName) -> Result<bool, CliError> {
        let mut disk = self.read()?;
        let Some(entries) = disk.get_mut(app.as_str()) else {
            return Ok(false);
        };
        let removed = entries.remove(name.as_str()).is_some();
        if entries.is_empty() {
            disk.remove(app.as_str());
        }
        if removed {
            self.write(&disk)?;
        }
        Ok(removed)
    }

    /// Resolve the intersection of what is asked for and what is held.
    ///
    /// **The caller passes what the manifest declared; this returns only names
    /// from that list.** A secret held here but absent from the manifest is
    /// never asked for and so cannot be returned — the property is structural
    /// rather than a check that could be forgotten, matching `SecretStore`'s
    /// contract and conformance R5.2.
    pub fn resolve(
        &self,
        app: &AppName,
        declared: &[SecretName],
    ) -> Result<Vec<(String, SecretValue)>, CliError> {
        let disk = self.read()?;
        let Some(entries) = disk.get(app.as_str()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for name in declared {
            if let Some(encoded) = entries.get(name.as_str()) {
                out.push((name.as_str().to_owned(), decode(encoded)?));
            }
        }
        Ok(out)
    }

    /// Read the file, or an empty store when it does not exist yet.
    fn read(&self) -> Result<Disk, CliError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Disk::new()),
            Err(e) => {
                return Err(CliError::Config {
                    what: format!("{} could not be read: {e}", self.path.display()),
                });
            }
        };
        serde_json::from_str(&text).map_err(|e| CliError::Config {
            what: format!("{} is not a valid secret store: {e}", self.path.display()),
        })
    }

    /// Write the file, creating it owner-only and verifying that it is.
    fn write(&self, disk: &Disk) -> Result<(), CliError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CliError::Config {
                what: format!("{} could not be created: {e}", parent.display()),
            })?;
            restrict(parent, DIR_MODE)?;
        }

        let mut text = serde_json::to_vec_pretty(disk).map_err(|e| CliError::Config {
            what: format!("the secret store could not be serialized: {e}"),
        })?;

        // Created with the mode set, not chmod'ed afterwards: between a create
        // and a later chmod there is a window where the file exists readable,
        // and a secret written into that window has already been exposed.
        write_private(&self.path, &text)?;
        wipe(&mut text);

        // Asserted rather than assumed. `create_new` honours the mode, but a
        // pre-existing file keeps its own, and a umask cannot widen a mode
        // while a previous run or an editor can.
        verify(&self.path, FILE_MODE)
    }
}

/// Encode a value for the file. Base64 because JSON has no byte type and the
/// WIT's `list<u8>` is not necessarily UTF-8.
fn encode(value: &SecretValue) -> String {
    base64::engine::general_purpose::STANDARD.encode(value.expose())
}

fn decode(encoded: &str) -> Result<SecretValue, CliError> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map(SecretValue::new)
        .map_err(|e| CliError::Config {
            what: format!("a stored secret is not valid base64: {e}"),
        })
}

/// Write `bytes` to `path` such that it is never readable by anyone else.
#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(|e| CliError::Config {
            what: format!("{} could not be opened for writing: {e}", path.display()),
        })?;
    file.write_all(bytes).map_err(|e| CliError::Config {
        what: format!("{} could not be written: {e}", path.display()),
    })
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    // Windows inherits the parent directory's ACL, and the per-user profile
    // directory is already owner-only. There is no mode to set.
    std::fs::write(path, bytes).map_err(|e| CliError::Config {
        what: format!("{} could not be written: {e}", path.display()),
    })
}

/// Narrow an existing path's permissions to `mode`.
#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| {
        CliError::Config {
            what: format!("{} could not be made private: {e}", path.display()),
        }
    })
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<(), CliError> {
    Ok(())
}

/// Fail if `path` is readable by anyone but its owner.
#[cfg(unix)]
fn verify(path: &Path, expected: u32) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(path)
        .map_err(|e| CliError::Config {
            what: format!("{} could not be checked: {e}", path.display()),
        })?
        .permissions()
        .mode()
        & 0o777;
    if mode != expected {
        return Err(CliError::Config {
            what: format!(
                "{} has mode {mode:o}, but a secret store must be {expected:o}. \
                 Fix it with `chmod {expected:o} {}`",
                path.display(),
                path.display()
            ),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify(_path: &Path, _expected: u32) -> Result<(), CliError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "clwd-secrets-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn store(&self) -> FileStore {
            FileStore::at(self.0.join("clwd").join("secrets.json"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app() -> AppName {
        AppName::new("dashboard").expect("valid")
    }

    fn name(n: &str) -> SecretName {
        SecretName::new(n).expect("valid")
    }

    #[test]
    fn a_value_round_trips() {
        let s = Scratch::new("roundtrip");
        let store = s.store();
        store
            .set(&app(), &name("token"), &SecretValue::new(b"abc".to_vec()))
            .expect("set");
        let got = store
            .get(&app(), &name("token"))
            .expect("get")
            .expect("held");
        assert_eq!(got.expose(), b"abc");
    }

    /// The WIT is `list<u8>`; a key or certificate must survive storage.
    #[test]
    fn a_non_utf8_value_round_trips() {
        let s = Scratch::new("bytes");
        let store = s.store();
        let raw = vec![0xff, 0x00, 0xfe, 0x80, 0x01];
        store
            .set(&app(), &name("key"), &SecretValue::new(raw.clone()))
            .expect("set");
        assert_eq!(
            store
                .get(&app(), &name("key"))
                .expect("get")
                .expect("held")
                .expose(),
            raw.as_slice()
        );
    }

    /// The permission claim, asserted on the artifact rather than assumed from
    /// the open flags. A umask cannot widen a mode, but a pre-existing file
    /// keeps its own, so this is the check that would catch a regression.
    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only_and_so_is_its_directory() {
        use std::os::unix::fs::PermissionsExt as _;
        let s = Scratch::new("modes");
        let store = s.store();
        store
            .set(&app(), &name("token"), &SecretValue::new(b"x".to_vec()))
            .expect("set");

        let file = std::fs::metadata(store.path())
            .expect("file exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file, 0o600, "file mode is {file:o}, expected 600");

        let dir = std::fs::metadata(store.path().parent().expect("has a parent"))
            .expect("dir exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir, 0o700, "directory mode is {dir:o}, expected 700");
    }

    /// A file someone widened is refused rather than used. Writing a secret
    /// into a world-readable file is the failure this store exists to prevent.
    #[cfg(unix)]
    #[test]
    fn a_world_readable_store_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let s = Scratch::new("widened");
        let store = s.store();
        store
            .set(&app(), &name("token"), &SecretValue::new(b"x".to_vec()))
            .expect("first set creates it");

        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644))
            .expect("widen");

        let err = store
            .set(&app(), &name("token"), &SecretValue::new(b"y".to_vec()))
            .expect_err("a widened store must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("644"),
            "the actual mode should be named: {msg}"
        );
        assert!(msg.contains("chmod"), "the fix should be named: {msg}");
    }

    /// The intersection property, R5.2: held but undeclared is never returned.
    #[test]
    fn a_held_secret_that_was_not_declared_is_not_resolved() {
        let s = Scratch::new("intersection");
        let store = s.store();
        store
            .set(
                &app(),
                &name("declared"),
                &SecretValue::new(b"yes".to_vec()),
            )
            .expect("set");
        store
            .set(
                &app(),
                &name("undeclared"),
                &SecretValue::new(b"no".to_vec()),
            )
            .expect("set");

        let resolved = store.resolve(&app(), &[name("declared")]).expect("resolve");
        let names: Vec<&str> = resolved.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["declared"]);
        assert!(
            !resolved.iter().any(|(_, v)| v.expose() == b"no"),
            "an undeclared secret's value reached the injection set"
        );
    }

    /// R5.3: declared but not held is absent, not an error.
    #[test]
    fn a_declared_secret_that_is_not_held_resolves_to_nothing() {
        let s = Scratch::new("missing");
        let store = s.store();
        let resolved = store.resolve(&app(), &[name("absent")]).expect("resolve");
        assert!(resolved.is_empty());
    }

    /// Namespaces do not leak into one another.
    #[test]
    fn one_apps_secrets_are_invisible_to_another() {
        let s = Scratch::new("namespaces");
        let store = s.store();
        let other = AppName::new("billing").expect("valid");
        store
            .set(&app(), &name("token"), &SecretValue::new(b"mine".to_vec()))
            .expect("set");

        assert!(store.get(&other, &name("token")).expect("get").is_none());
        assert!(store.list(&other).expect("list").is_empty());
    }

    #[test]
    fn listing_yields_names_only_and_is_sorted() {
        let s = Scratch::new("list");
        let store = s.store();
        for n in ["zebra", "alpha", "middle"] {
            store
                .set(&app(), &name(n), &SecretValue::new(b"v".to_vec()))
                .expect("set");
        }
        let listed: Vec<String> = store
            .list(&app())
            .expect("list")
            .iter()
            .map(|n| n.as_str().to_owned())
            .collect();
        assert_eq!(listed, ["alpha", "middle", "zebra"]);
    }

    /// Idempotent, matching app-data's delete.
    #[test]
    fn removing_what_is_not_there_succeeds() {
        let s = Scratch::new("remove");
        let store = s.store();
        assert!(!store.remove(&app(), &name("nothing")).expect("remove"));

        store
            .set(&app(), &name("token"), &SecretValue::new(b"v".to_vec()))
            .expect("set");
        assert!(store.remove(&app(), &name("token")).expect("remove"));
        assert!(store.get(&app(), &name("token")).expect("get").is_none());
    }

    /// A store that does not exist is empty, not an error: the first `list`
    /// on a fresh machine should say "nothing", not fail.
    #[test]
    fn a_store_that_does_not_exist_yet_reads_as_empty() {
        let s = Scratch::new("absent");
        let store = s.store();
        assert!(store.list(&app()).expect("list").is_empty());
        assert!(store.get(&app(), &name("x")).expect("get").is_none());
    }

    /// Byte-stable across writes, so a diff of the file means something.
    #[test]
    fn the_file_is_written_in_a_stable_order() {
        let s = Scratch::new("stable");
        let store = s.store();
        for n in ["b", "a", "c"] {
            store
                .set(&app(), &name(n), &SecretValue::new(b"v".to_vec()))
                .expect("set");
        }
        let first = std::fs::read(store.path()).expect("read");
        store
            .set(&app(), &name("a"), &SecretValue::new(b"v".to_vec()))
            .expect("rewrite an existing value unchanged");
        assert_eq!(first, std::fs::read(store.path()).expect("read"));
    }
}
