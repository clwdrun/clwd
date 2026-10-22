//! Sandbox construction shared by `clwd run` and `clwd dev`.
//!
//! Both commands run an app locally against the same posture: in-memory stores,
//! a `local@local` caller, and — when `clwd.toml` names the app — the persistent
//! local secret store keyed by that name. Building the sandbox in one place is
//! what keeps them from drifting: a capability-enforcement fix (`#33`) or an
//! identity change lands once and reaches both, rather than in two constructors
//! that only happen to agree.
//!
//! Everything here is behind `feature = "run"`, because it links the runtime.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use clwd_bundle::BundleRef;
use clwd_runtime::wasm::{
    AppDataStore, AppId, AppIdResolver, DigestAsAppId, MemoryAppDataStore, MemoryBundleStore,
    MemorySecretStore, SecretStore, WasmSandbox,
};
use clwd_runtime::{Caller, IdentityCtx, OrgId, Principal, SubjectId};

use crate::error::CliError;
use crate::secrets::{AppName, FileStore, SecretValue};

/// The subject a local run presents.
///
/// Not anonymous and not configurable. `#29`'s templates show authors reading
/// `ctx().caller()`; against an anonymous caller they would learn the anonymous
/// branch is the normal case, when in production it is the share-link case. A
/// local run is a signed-in user — obviously not a real one.
pub(crate) const LOCAL_SUBJECT: &str = "local";

/// The org a local run presents. On the platform's reserved list, so it can
/// never collide with a real org slug.
pub(crate) const LOCAL_ORG: &str = "local";

/// The email a local run presents. `.localhost` is reserved, so this can never
/// be someone's real address, and a template that greets by email has
/// something to show locally.
pub(crate) const LOCAL_EMAIL: &str = "local@localhost";

/// The caller a local run presents. See [`LOCAL_SUBJECT`].
pub(crate) fn local_identity() -> IdentityCtx {
    let mut principal = Principal::new(SubjectId::new(LOCAL_SUBJECT), OrgId::new(LOCAL_ORG));
    principal.email = Some(LOCAL_EMAIL.to_owned());
    IdentityCtx::new(Caller::User(principal))
}

/// Read bundle bytes, rejecting a bare component with the fix.
pub(crate) fn read_bundle(path: &Path) -> Result<Vec<u8>, CliError> {
    if path.extension().is_some_and(|e| e == "wasm") {
        return Err(CliError::InvalidArgument {
            what: format!(
                "{} is a component, not a bundle. A local run needs the manifest that \
                 declares the app's limits and permissions, which a bare .wasm has not \
                 got; running without one is a different security posture from running \
                 with sealed limits",
                path.display()
            ),
        });
    }

    std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CliError::NotFound {
            what: format!("no bundle at {}", path.display()),
        },
        _ => CliError::Internal {
            source: anyhow::Error::new(e).context(format!("reading {}", path.display())),
        },
    })
}

/// Delegate a runtime failure with its code intact.
pub(crate) fn from_runtime(source: clwd_runtime::Error) -> CliError {
    CliError::Runtime { source }
}

/// Surface a bundle failure with its own code intact.
pub(crate) fn from_bundle(source: clwd_bundle::Error) -> CliError {
    CliError::Bundle { source }
}

/// A sandbox whose every store lives in this process and dies with it.
///
/// Secrets come from the local file store when the project names an app, and are
/// empty otherwise. The secret store and the app identity change together: the
/// local store is persistent, so keying it on a digest that moves every build
/// would orphan every secret on each rebuild — see [`DeclaredAppId`].
pub(crate) fn local_sandbox(
    digest: &BundleRef,
    bytes: Vec<u8>,
    project: &Path,
) -> Result<WasmSandbox, CliError> {
    local_sandbox_with(digest, bytes, project, Arc::new(MemoryAppDataStore::new()))
}

/// [`local_sandbox`] with the app-data store supplied, so a test can watch
/// every read and write the app makes.
pub(crate) fn local_sandbox_with(
    digest: &BundleRef,
    bytes: Vec<u8>,
    project: &Path,
    data: Arc<dyn AppDataStore>,
) -> Result<WasmSandbox, CliError> {
    let bundles = MemoryBundleStore::new();
    bundles.insert(digest, bytes);

    let stores = match local_stores(project)? {
        Some(stores) => stores,
        None => LocalStores {
            secrets: Arc::new(MemorySecretStore::new()),
            identity: Arc::new(DigestAsAppId),
        },
    };

    WasmSandbox::with_stores(Arc::new(bundles), stores.secrets, data, stores.identity)
        .map_err(from_runtime)
}

// ---------------------------------------------------------------------------
// Local secret store + app identity.
//
// The default `DigestAsAppId` is correct only while the stores paired with it
// are ephemeral: a digest changes on every build, so a persistent store keyed
// that way would move the app into an empty namespace on each rebuild and orphan
// everything in it. The local secret store is persistent, so it is keyed by the
// app's declared name from `[app] name`, and `DeclaredAppId` makes the sandbox
// agree — the identity the runtime believes in and the identity the secrets live
// under are the same string.
// ---------------------------------------------------------------------------

/// Resolves a guest's secrets from the local development store.
///
/// Loaded once, at construction, so `SecretStore::resolve` is infallible and a
/// malformed secret file becomes a `cli.*` error naming it before a sandbox
/// exists rather than an opaque `runtime.*` failure at spawn.
#[derive(Debug)]
pub(crate) struct LocalSecretStore {
    held: BTreeMap<String, SecretValue>,
}

impl LocalSecretStore {
    /// Read every secret held for `app`.
    pub(crate) fn new(app: &AppName) -> Result<Self, CliError> {
        Ok(Self {
            held: FileStore::open()?.load(app)?,
        })
    }
}

#[async_trait]
impl SecretStore for LocalSecretStore {
    /// Return the intersection of what the manifest declared and what is held.
    ///
    /// `names` is what the manifest declared; iterating it and looking each up —
    /// never the reverse — means a held-but-undeclared secret is never visited,
    /// so there is no path from the guest to it (conformance R5.2, structural).
    async fn resolve(
        &self,
        _app: &AppId,
        names: &[String],
    ) -> clwd_runtime::Result<Vec<(String, Vec<u8>)>> {
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

/// Uses the app's declared name as its identity, so a rebuild does not move the
/// app to a new identity. See the section comment above.
#[derive(Debug)]
pub(crate) struct DeclaredAppId(AppId);

impl DeclaredAppId {
    #[must_use]
    pub(crate) fn new(app: &AppName) -> Self {
        Self(AppId::new(app.as_str().to_owned()))
    }
}

#[async_trait]
impl AppIdResolver for DeclaredAppId {
    async fn resolve(&self, _bundle: &BundleRef) -> clwd_runtime::Result<AppId> {
        Ok(self.0.clone())
    }
}

/// The secret store and app identity a local run installs together.
pub(crate) struct LocalStores {
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) identity: Arc<dyn AppIdResolver>,
}

/// The stores a local run should use, given what the project declares.
///
/// `None` when the project does not name an app, leaving the caller on the
/// ephemeral defaults: an unnamed project getting no secrets is the correct
/// posture, not a degraded one — there is no namespace to read from.
pub(crate) fn local_stores(project: &Path) -> Result<Option<LocalStores>, CliError> {
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
