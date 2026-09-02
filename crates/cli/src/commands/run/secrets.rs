//! Making the local secret store visible to a running guest.
//!
//! # Why the `AppId` argument is ignored
//!
//! [`SecretStore::resolve`] is handed an `AppId` produced by the sandbox's
//! [`AppIdResolver`]. This store ignores it, and that is the whole point.
//!
//! The default resolver is `DigestAsAppId`, whose own documentation says it is
//! correct **only** while the stores paired with it are ephemeral: a digest
//! changes on every build, so a persistent store keyed that way would move the
//! app into an empty namespace on each rebuild and orphan everything in it —
//! silently, looking like the app forgot.
//!
//! The local secret store is persistent. Keyed by digest it would lose every
//! secret the moment the app was rebuilt, which is exactly the failure that
//! comment warns about. So it is keyed by the app's declared name, taken from
//! `[app] name` in `clwd.toml`, and it carries that name itself rather than
//! taking it from an argument that means something else.
//!
//! [`DeclaredAppId`] then makes the sandbox agree, so the identity the runtime
//! believes in and the identity the secrets live under are the same string
//! rather than two things that happen to work.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use clwd_bundle::BundleRef;
use clwd_runtime::wasm::{AppId, AppIdResolver, SecretStore};

use crate::error::CliError;
use crate::secrets::{AppName, FileStore, SecretValue};

/// Resolves a guest's secrets from the local development store.
///
/// Loaded once, at construction. `SecretStore::resolve` can only fail with a
/// `runtime.*` code and none of them means "this machine's secret file is
/// malformed", so reading it here turns that into a `cli.*` error naming the
/// file, raised before a sandbox exists. It also bounds the work at one file
/// read per run.
#[derive(Debug)]
pub struct LocalSecretStore {
    held: BTreeMap<String, SecretValue>,
}

impl LocalSecretStore {
    /// Read every secret held for `app`.
    pub fn new(app: &AppName) -> Result<Self, CliError> {
        Ok(Self {
            held: FileStore::open()?.load(app)?,
        })
    }
}

#[async_trait]
impl SecretStore for LocalSecretStore {
    /// Return the intersection of what the manifest declared and what is held.
    ///
    /// `names` is what the manifest declared; nothing outside it can be
    /// returned, because nothing outside it is looked up. A secret sitting in
    /// the local store that the manifest does not name is never asked for, so
    /// there is no path from the guest to it — conformance R5.2, structurally
    /// rather than by a check that could be removed.
    ///
    /// Infallible, because the file was already read at construction. A
    /// spawn cannot fail here on a problem that has nothing to do with the
    /// guest.
    async fn resolve(
        &self,
        _app: &AppId,
        names: &[String],
    ) -> clwd_runtime::Result<Vec<(String, Vec<u8>)>> {
        // Iterating what was DECLARED and looking each up, never the reverse.
        // A secret held here but absent from `names` is not visited, so there
        // is no path from the guest to it.
        //
        // `expose` is deliberate and is one of only two places in the CLI that
        // touch a value: this is the host handing bytes to the instance, which
        // is the entire reason the store exists.
        Ok(names
            .iter()
            .filter_map(|name| {
                self.held
                    .get(name)
                    .map(|value| (name.clone(), value.expose().to_vec()))
            })
            .collect())
    }
}

/// Uses the app's declared name as its identity.
///
/// Replaces `DigestAsAppId` when `clwd.toml` names the app, so a rebuild does
/// not move the app to a new identity. See the module docs.
#[derive(Debug)]
pub struct DeclaredAppId(AppId);

impl DeclaredAppId {
    #[must_use]
    pub fn new(app: &AppName) -> Self {
        Self(AppId::new(app.as_str().to_owned()))
    }
}

#[async_trait]
impl AppIdResolver for DeclaredAppId {
    async fn resolve(&self, _bundle: &BundleRef) -> clwd_runtime::Result<AppId> {
        Ok(self.0.clone())
    }
}

/// The pair a local run installs together.
///
/// A struct rather than a tuple because the two must change together and the
/// names say which is which: a persistent secret store keyed by a digest that
/// moves every build is exactly the orphaning failure `DigestAsAppId` warns
/// about, so the identity travels with the store that needs it.
pub struct LocalStores {
    pub secrets: Arc<dyn SecretStore>,
    pub identity: Arc<dyn AppIdResolver>,
}

/// The stores a run should use, given what the project declares.
///
/// Returns `None` when the project does not name an app, leaving the caller on
/// the ephemeral defaults. An unnamed project getting no secrets is the correct
/// local posture, not a degraded one: there is no namespace to read from.
pub fn local_stores(project: &std::path::Path) -> Result<Option<LocalStores>, CliError> {
    let manifest = project.join(crate::build::manifest::FILENAME);
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Ok(None);
    };
    let source = crate::build::Source::parse(&text)?;
    let Some(name) = source.app_name() else {
        return Ok(None);
    };
    let app = AppName::new(name)?;
    Ok(Some(LocalStores {
        secrets: Arc::new(LocalSecretStore::new(&app)?),
        identity: Arc::new(DeclaredAppId::new(&app)),
    }))
}
