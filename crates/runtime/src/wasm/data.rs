//! Where secrets and app data come from.
//!
//! These traits are the seam. The production stores live in the platform and
//! reach this crate across a tag-pinned dependency; what is here is enough to
//! run and test locally, and is built so that it cannot become the production
//! implementation by accident.
//!
//! No platform concept appears in any signature — no org, no grant, no tenant.
//! The only identifiers are [`clwd_bundle::BundleRef`], which is ours, and
//! [`AppId`], which an implementation produces.

use parking_lot::Mutex;
use std::collections::HashMap;

use async_trait::async_trait;
use clwd_bundle::BundleRef;

use crate::error::Result;

/// A stable identity for an application, across every version of it.
///
/// **Not a [`BundleRef`].** A `BundleRef` is a digest over one artifact, so
/// every deploy produces a different one. An app's data must survive its
/// deploys, which means the namespace it lives in cannot be derived from the
/// thing that changes on every deploy.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppId(String);

impl AppId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AppId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Maps an artifact to the application it is a version of.
///
/// This exists as its own trait, rather than the data store simply taking a
/// `BundleRef`, so that using a digest as an app identity is a **deliberate
/// choice with a name** rather than the path of least resistance. See
/// [`DigestAsAppId`] for what that choice costs.
///
/// # Implementations may and should cache
///
/// The mapping from an artifact to its application is expected to be **stable
/// for the lifetime of that artifact** — a given digest is a version of exactly
/// one app, forever. A caching implementation is therefore correct, not a
/// shortcut, and callers should assume nothing about how often this is invoked.
///
/// That matters for latency. Under the wasm backend's per-invocation
/// instantiation, resolution sits on the path of *every* request, not just the
/// first. An implementation that makes a network call here without caching adds
/// that round trip to a request budget currently measured in the low hundreds
/// of microseconds.
#[async_trait]
pub trait AppIdResolver: Send + Sync {
    async fn resolve(&self, bundle: &BundleRef) -> Result<AppId>;
}

/// Uses the artifact digest directly as the application identity.
///
/// **Correct only because the store paired with it is in-memory and
/// ephemeral.** A digest changes on every deploy, so a *persistent* store keyed
/// this way would move the app to an empty namespace on each release and orphan
/// everything its users had stored — silently, looking like the app forgot, and
/// unrecoverably once the old artifact is collected.
///
/// Local development and tests never redeploy into the same process, so nothing
/// is lost there. Anything durable needs a real mapping; that is `platform#21`.
#[derive(Debug, Default)]
pub struct DigestAsAppId;

#[async_trait]
impl AppIdResolver for DigestAsAppId {
    async fn resolve(&self, bundle: &BundleRef) -> Result<AppId> {
        Ok(AppId::new(bundle.to_string()))
    }
}

/// Resolves the secrets an application is allowed to hold.
///
/// Consulted once per spawn, before the guest runs. The names asked for are the
/// ones the bundle manifest declared; what comes back is what this store is
/// willing to authorise. **The manifest declares and the store authorises: the
/// injection set is the intersection.** A name the store does not return is
/// simply absent from the instance, with no code path from the guest to it.
///
/// Returning fewer entries than were asked for is normal and is not an error.
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn resolve(&self, app: &AppId, names: &[String]) -> Result<Vec<(String, Vec<u8>)>>;
}

/// A per-application key-value store.
///
/// Every operation is scoped by an [`AppId`] the host supplies. A guest cannot
/// name, forge or influence that value — it never crosses the interface — which
/// is what makes one shared store safe to serve every instance.
#[async_trait]
pub trait AppDataStore: Send + Sync {
    async fn get(&self, app: &AppId, key: &str) -> Result<Option<Vec<u8>>>;
    async fn set(&self, app: &AppId, key: &str, value: Vec<u8>) -> Result<()>;
    async fn delete(&self, app: &AppId, key: &str) -> Result<()>;
}

/// Secrets held in memory, for local development and tests.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    secrets: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Authorise `name` for `app`.
    pub fn insert(&self, app: &AppId, name: &str, value: impl Into<Vec<u8>>) {
        self.secrets
            .lock()
            .insert((app.as_str().to_owned(), name.to_owned()), value.into());
    }
}

#[async_trait]
impl SecretStore for MemorySecretStore {
    async fn resolve(&self, app: &AppId, names: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
        let secrets = self.secrets.lock();
        // Only what was asked for AND is held: a declared name this store will
        // not authorise yields nothing rather than an error, so a manifest can
        // narrow the set but never widen it.
        Ok(names
            .iter()
            .filter_map(|name| {
                secrets
                    .get(&(app.as_str().to_owned(), name.clone()))
                    .map(|value| (name.clone(), value.clone()))
            })
            .collect())
    }
}

/// App data held in memory, for local development and tests.
///
/// Shared across every instance by design: data that did not outlive a single
/// invocation would not be persistence. Safety comes from every operation being
/// scoped by an [`AppId`] the guest cannot supply or alter, not from the map
/// being unshared.
#[derive(Debug, Default)]
pub struct MemoryAppDataStore {
    data: Mutex<HashMap<AppId, HashMap<String, Vec<u8>>>>,
}

impl MemoryAppDataStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many keys `app` currently holds. For tests asserting that one app's
    /// namespace is untouched by another's writes.
    pub fn len(&self, app: &AppId) -> usize {
        self.data.lock().get(app).map_or(0, HashMap::len)
    }

    pub fn is_empty(&self, app: &AppId) -> bool {
        self.len(app) == 0
    }
}

#[async_trait]
impl AppDataStore for MemoryAppDataStore {
    async fn get(&self, app: &AppId, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .data
            .lock()
            .get(app)
            .and_then(|keys| keys.get(key))
            .cloned())
    }

    async fn set(&self, app: &AppId, key: &str, value: Vec<u8>) -> Result<()> {
        self.data
            .lock()
            .entry(app.clone())
            .or_default()
            .insert(key.to_owned(), value);
        Ok(())
    }

    async fn delete(&self, app: &AppId, key: &str) -> Result<()> {
        // Idempotent, per the interface: removing what is not there succeeds.
        if let Some(keys) = self.data.lock().get_mut(app) {
            keys.remove(key);
        }
        Ok(())
    }
}

/// The key rules from `clwd:app-data`: 1 to 128 bytes of `a`-`z`, `0`-`9`,
/// `.`, `_` or `-`, not beginning or ending with a separator.
///
/// Enforced at the host rather than left to each store, so every backend sees
/// the same key space and a key that works locally works everywhere.
pub(super) fn validate_key(key: &str) -> std::result::Result<(), String> {
    if key.is_empty() {
        return Err("key is empty".to_owned());
    }
    if key.len() > 128 {
        return Err(format!("key is {} bytes; the limit is 128", key.len()));
    }
    if let Some(bad) = key
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')))
    {
        return Err(format!(
            "key contains {bad:?}; only a-z, 0-9, '.', '_' and '-' are allowed"
        ));
    }
    let separator = |c: char| matches!(c, '.' | '_' | '-');
    if key.starts_with(separator) || key.ends_with(separator) {
        return Err("key begins or ends with a separator".to_owned());
    }
    Ok(())
}

/// The smallest value every host must accept, from `clwd:app-data`.
pub(super) const MIN_VALUE_LIMIT: u64 = 1024 * 1024;
