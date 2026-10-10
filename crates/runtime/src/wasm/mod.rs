//! wasmtime-backed `Sandbox` for short-lived, request/response apps.
//!
//! Component model plus `wasmtime-wasi-http`. Epoch interruption is configured
//! here but its budget belongs to #17.
//!
//! # Isolation: what this backend upholds by construction
//!
//! [`crate::Sandbox`] states the caller-isolation invariant and names one thing
//! its signatures cannot enforce: a backend that pools guests internally could
//! alias two handles onto one instance. **This backend cannot do that, and not
//! because it is careful.**
//!
//! `invoke` builds a fresh `Store` and instantiates the component for each
//! request, from the [`crate::IdentityCtx`] that handle was spawned with. An
//! instance exists only for the duration of one call, so there is no
//! long-lived guest for a second handle to be pointed at. The failure is not
//! avoided; it is unrepresentable — the same subtractive move the trait makes
//! one layer up.
//!
//! Instantiation from a cached component measures ~40µs against ~100µs of guest
//! execution, so this costs well under half the work the request does anyway.
//!
//! ## A non-guarantee that becomes a guarantee here
//!
//! The trait documents "state leaking between a caller's own sequential
//! requests" as something a backend must avoid by discipline. **This backend
//! promises it instead**: every invocation gets fresh linear memory, so nothing
//! a guest writes survives into its next request.
//!
//! That is a property of *this backend*, not of the trait. The microVM backend
//! keeps a long-lived VM by design and will not offer it. Do not read this
//! paragraph as something `dyn Sandbox` guarantees.
//!
//! # What is still upheld by discipline
//!
//! - **The host must not hand one `Store` to two callers.** Nothing here does,
//!   and the `Store` is built inside `invoke` from the handle's own identity —
//!   but it is code, not a type, that keeps it so.
//! - **Concurrent invocations on one handle** each get their own `Store`, so
//!   they cannot observe each other. That falls out of the model rather than
//!   being arranged.

mod budget;
mod cache;
mod capability;
mod data;
mod engine;
mod epoch;
mod host;
mod invoke;
mod limits;
mod logs;

use std::collections::HashMap;
use std::sync::Arc;

use crate::Sandbox;
use crate::call::{InvokeOptions, Request, Response, SpawnOptions};
use crate::error::{Error, Phase, Result};
use crate::handle::{Handle, InstanceId};
use crate::identity::IdentityCtx;
use crate::logs::LogStream;
use async_trait::async_trait;
use clwd_bundle::BundleRef;
use parking_lot::Mutex;

use budget::Budget;
use cache::ComponentCache;
use capability::{EgressPolicy, StoragePolicy};
pub use data::{
    AppDataStore, AppId, AppIdResolver, DigestAsAppId, MemoryAppDataStore, MemorySecretStore,
    SecretStore,
};
use logs::InstanceLogs;

/// Where a backend reads bundle bytes from.
///
/// The runtime resolves a digest to bytes; it does not decide where artifacts
/// live. A caller supplies this so the same backend serves a test with an
/// in-memory map and a host with a real store.
#[async_trait]
pub trait BundleStore: Send + Sync {
    async fn fetch(&self, bundle: &BundleRef) -> Result<Vec<u8>>;
}

/// An in-memory store, for tests and for a host that already holds its bytes.
#[derive(Debug, Default)]
pub struct MemoryBundleStore {
    artifacts: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryBundleStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, bundle: &BundleRef, bytes: Vec<u8>) {
        self.artifacts.lock().insert(bundle.to_string(), bytes);
    }
}

#[async_trait]
impl BundleStore for MemoryBundleStore {
    async fn fetch(&self, bundle: &BundleRef) -> Result<Vec<u8>> {
        self.artifacts
            .lock()
            .get(&bundle.to_string())
            .cloned()
            .ok_or(Error::BundleNotFound { bundle: *bundle })
    }
}

struct Instance {
    bundle: BundleRef,
    /// The application this artifact is a version of, resolved once at spawn.
    app: AppId,
    /// Every secret this instance may see, resolved before it ran.
    ///
    /// Held here rather than re-resolved per request: under per-invocation
    /// instantiation a fresh `Store` is built for every call, and re-consulting
    /// the store each time would put an I/O round trip on every request and
    /// let the set change underneath a running instance.
    secrets: HashMap<String, Vec<u8>>,
    /// Resolved once, from the manifest the digest sealed. Kept here so
    /// `invoke` needs no second decode on the cache-hit path.
    budget: Budget,
    /// The outbound-egress allowlist, precomputed once at spawn from the
    /// manifest. Deny-by-default: an empty grant denies every host.
    egress: EgressPolicy,
    /// The storage-namespace allowlist, precomputed once at spawn. Deny-by-default.
    storage: StoragePolicy,
    logs: Arc<InstanceLogs>,
}

/// The wasmtime backend.
pub struct WasmSandbox {
    cache: ComponentCache,
    store: Arc<dyn BundleStore>,
    secrets: Arc<dyn SecretStore>,
    app_data: Arc<dyn AppDataStore>,
    apps: Arc<dyn AppIdResolver>,
    instances: Mutex<HashMap<InstanceId, Instance>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for WasmSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmSandbox").finish_non_exhaustive()
    }
}

impl WasmSandbox {
    /// A sandbox with in-memory stores, for local development and tests.
    ///
    /// The defaults are deliberately ephemeral. In particular the app identity
    /// is the artifact digest, which is only sound because nothing here
    /// outlives the process — see [`DigestAsAppId`].
    pub fn new(store: Arc<dyn BundleStore>) -> Result<Self> {
        Self::with_stores(
            store,
            Arc::new(MemorySecretStore::new()),
            Arc::new(MemoryAppDataStore::new()),
            Arc::new(DigestAsAppId),
        )
    }

    /// A sandbox with stores the caller supplies. This is the seam the platform
    /// reaches through.
    pub fn with_stores(
        store: Arc<dyn BundleStore>,
        secrets: Arc<dyn SecretStore>,
        app_data: Arc<dyn AppDataStore>,
        apps: Arc<dyn AppIdResolver>,
    ) -> Result<Self> {
        let engine = engine::build()?;
        // Nothing enforces a deadline until something advances the epoch.
        epoch::spawn(&engine);
        Ok(Self {
            cache: ComponentCache::new(engine)?,
            store,
            secrets,
            app_data,
            apps,
            instances: Mutex::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Resident compiled components. Tests assert on this instead of inferring
    /// cache behaviour from timing.
    pub async fn cached_components(&self) -> usize {
        self.cache.len()
    }

    /// How many times a component has actually been compiled.
    ///
    /// Distinct from [`Self::cached_components`]: the resident count reaches
    /// one whether the single-flight path worked or two threads compiled the
    /// same bundle and one result was discarded. Only this number can tell
    /// those apart, which is why the concurrency test asserts on it.
    pub fn compilations(&self) -> u64 {
        self.cache.compilations()
    }

    /// Test seam (`feature = "conformance"`): make the instance behind `id`
    /// report `runtime.stopped` on its next invoke, without a `Handle`.
    ///
    /// The conformance suite drives R2.1/R2.2 — an invoke after the instance is
    /// gone — while still holding a live handle to invoke with. Production
    /// teardown is [`Sandbox::stop`], which consumes the handle, so this path
    /// is unreachable there and not part of the public surface.
    #[cfg(feature = "conformance")]
    pub async fn evict(&self, id: &InstanceId) {
        self.instances.lock().remove(id);
    }

    /// Instances spawned and not yet stopped. Every spawn holds a copy of its
    /// app's secrets, so a host that spawns per request leaks both unless
    /// `stop` removes the entry; tests assert this count rather than the
    /// absence of a crash.
    pub fn live_instances(&self) -> usize {
        self.instances.lock().len()
    }

    /// Resolve a digest to the bytes of the component to run.
    ///
    /// **Do not inline this at the call sites.** It is one function because it
    /// guards two things that must not come apart:
    ///
    /// 1. **A bundle is a tar archive, not a component.** What wasmtime
    ///    compiles is the member `app.entrypoint` names. Handing it the archive
    ///    instead does not fail cleanly — wasmtime decides the bytes are not
    ///    binary wasm, falls back to parsing them as WAT text, and reports
    ///    `input bytes aren't valid utf-8`, which names neither the real
    ///    problem nor the archive.
    /// 2. **Decoding is the ingest-side validation gate.** It verifies the
    ///    digest and rejects a core module before wasmtime gets its second
    ///    look. Extracting a member without decoding first would skip it.
    ///
    /// Separating them puts the extraction one edit away from being done
    /// without the validation, or the validation one edit away from being
    /// dropped as redundant.
    async fn entrypoint(
        &self,
        bundle: &BundleRef,
    ) -> Result<(Vec<u8>, Budget, Vec<String>, EgressPolicy, StoragePolicy)> {
        let bytes = self.store.fetch(bundle).await?;
        let decoded = clwd_bundle::decode(&bytes, Some(*bundle))?;
        let budget = Budget::from_manifest(&decoded.manifest.capabilities.limits);
        // The names the manifest declares. What the store will authorise of
        // these is decided at spawn; the manifest can narrow, never grant.
        let declared = decoded.manifest.capabilities.secrets.clone();
        // The egress and storage allowlists, compiled once here from the sealed
        // manifest so the per-request checks parse and resolve nothing.
        let egress = EgressPolicy::from_manifest(&decoded.manifest.capabilities.egress);
        let storage =
            StoragePolicy::from_manifest(&decoded.manifest.capabilities.storage_namespaces);
        let entrypoint = &decoded.manifest.entrypoint;
        let data = decoded
            .members
            .into_iter()
            .find(|m| &m.path == entrypoint)
            .map(|m| m.data)
            .ok_or_else(|| Error::Internal {
                source: format!("entrypoint {entrypoint:?} missing after validation").into(),
            })?;
        Ok((data, budget, declared, egress, storage))
    }
}

#[async_trait]
impl Sandbox for WasmSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        let deadline = opts.deadline;
        let (bytes, budget, declared_secrets, egress, storage) = self.entrypoint(&bundle).await?;

        let expired = || Error::DeadlineExceeded {
            phase: Phase::Spawn,
            limit: std::time::Duration::ZERO,
        };
        if deadline.is_some_and(|d| d.is_expired()) {
            return Err(expired());
        }

        // Compilation dominates cold start, so it is what the deadline guards.
        let compile = self.cache.get_or_compile(&bundle, &bytes);
        match deadline {
            Some(d) => tokio::time::timeout(d.remaining(), compile)
                .await
                .map_err(|_| Error::DeadlineExceeded {
                    phase: Phase::Spawn,
                    limit: d.remaining(),
                })??,
            None => compile.await?,
        };

        // Resolve identity and secrets before the instance exists. A failure
        // here means it never starts, which is what makes `secrets.get`
        // infallible at call time.
        let app = self.apps.resolve(&bundle).await?;
        let secrets: HashMap<String, Vec<u8>> = self
            .secrets
            .resolve(&app, &declared_secrets)
            .await?
            .into_iter()
            .collect();

        let id = InstanceId::new(format!(
            "wasm-{}",
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        self.instances.lock().insert(
            id.clone(),
            Instance {
                bundle,
                app,
                secrets,
                budget,
                egress,
                storage,
                logs: Arc::new(InstanceLogs::default()),
            },
        );
        Ok(Handle::new(ctx, id))
    }

    async fn invoke(&self, handle: &Handle, req: Request, opts: InvokeOptions) -> Result<Response> {
        let (bundle, budget, app, secrets, egress, storage, logs) = {
            let instances = self.instances.lock();
            let instance = instances.get(handle.instance()).ok_or(Error::Stopped {})?;
            (
                instance.bundle,
                instance.budget,
                instance.app.clone(),
                instance.secrets.clone(),
                instance.egress.clone(),
                instance.storage.clone(),
                Arc::clone(&instance.logs),
            )
        };

        // An already-spent budget means do not start, the same as `spawn`.
        // `tokio::time::timeout` alone is not enough: it polls the inner future
        // before checking the clock, so a fast guest completes and wins the
        // race against a deadline that had already passed.
        if opts.deadline.is_some_and(|d| d.is_expired()) {
            return Err(Error::DeadlineExceeded {
                phase: Phase::Invoke,
                limit: std::time::Duration::ZERO,
            });
        }

        // The component is almost always resident: `spawn` put it there. Only
        // resolve bytes on a miss — resolving decodes the bundle, which
        // re-verifies every member digest, and repeating that per request costs
        // time proportional to bundle size for an artifact already validated.
        let component = match self.cache.get(&bundle) {
            Some(prepared) => prepared,
            None => {
                let (bytes, ..) = self.entrypoint(&bundle).await?;
                self.cache.get_or_compile(&bundle, &bytes).await?
            }
        };

        let call = invoke::run(
            self.cache.engine(),
            &component,
            invoke::InstanceContext {
                identity: handle.identity().clone(),
                budget,
                secrets,
                app,
                app_data: Arc::clone(&self.app_data),
                egress,
                storage,
            },
            req,
            opts.request_id.clone(),
            Arc::clone(&logs),
        );

        match opts.deadline {
            Some(d) => tokio::time::timeout(d.remaining(), call)
                .await
                .map_err(|_| Error::DeadlineExceeded {
                    phase: Phase::Invoke,
                    limit: d.remaining(),
                })?,
            None => call.await,
        }
    }

    async fn logs_stream(&self, handle: &Handle) -> Result<LogStream> {
        let instances = self.instances.lock();
        let instance = instances.get(handle.instance()).ok_or(Error::Stopped {})?;
        Ok(LogStream::new(instance.logs.subscribe()))
    }

    async fn stop(&self, handle: Handle) -> Result<()> {
        // Removed, not flagged: the entry holds the app's secrets, and a host
        // that spawns per request would otherwise keep every one it ever ran.
        self.instances.lock().remove(handle.instance());
        Ok(())
    }
}
