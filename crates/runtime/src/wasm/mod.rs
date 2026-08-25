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

mod cache;
mod engine;
mod host;
mod invoke;
mod logs;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use clwd_bundle::BundleRef;
use tokio::sync::Mutex;
use crate::call::{InvokeOptions, Request, Response, SpawnOptions};
use crate::error::{Error, Phase, Result};
use crate::handle::{Handle, InstanceId};
use crate::identity::IdentityCtx;
use crate::logs::LogStream;
use crate::Sandbox;

use cache::ComponentCache;
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
    artifacts: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryBundleStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, bundle: &BundleRef, bytes: Vec<u8>) {
        self.artifacts
            .lock()
            .expect("not poisoned")
            .insert(bundle.to_string(), bytes);
    }
}

#[async_trait]
impl BundleStore for MemoryBundleStore {
    async fn fetch(&self, bundle: &BundleRef) -> Result<Vec<u8>> {
        self.artifacts
            .lock()
            .expect("not poisoned")
            .get(&bundle.to_string())
            .cloned()
            .ok_or(Error::BundleNotFound { bundle: *bundle })
    }
}

struct Instance {
    bundle: BundleRef,
    logs: Arc<InstanceLogs>,
    stopped: bool,
}

/// The wasmtime backend.
pub struct WasmSandbox {
    cache: ComponentCache,
    store: Arc<dyn BundleStore>,
    instances: Mutex<HashMap<InstanceId, Instance>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for WasmSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmSandbox").finish_non_exhaustive()
    }
}

impl WasmSandbox {
    pub fn new(store: Arc<dyn BundleStore>) -> Result<Self> {
        Ok(Self {
            cache: ComponentCache::new(engine::build()?)?,
            store,
            instances: Mutex::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Resident compiled components. Tests assert on this instead of inferring
    /// cache behaviour from timing.
    pub async fn cached_components(&self) -> usize {
        self.cache.len().await
    }

    /// Resolve a digest to the bytes of the component to run.
    ///
    /// A bundle is a tar archive, not a component: what wasmtime compiles is
    /// the member `app.entrypoint` names. Decoding also validates, so a core
    /// module is rejected here — `clwd-bundle`'s ingest gate — before wasmtime
    /// gets a second look at it.
    async fn entrypoint(&self, bundle: &BundleRef) -> Result<Vec<u8>> {
        let bytes = self.store.fetch(bundle).await?;
        let decoded = clwd_bundle::decode(&bytes, Some(*bundle))?;
        let entrypoint = &decoded.manifest.entrypoint;
        decoded
            .members
            .into_iter()
            .find(|m| &m.path == entrypoint)
            .map(|m| m.data)
            .ok_or_else(|| Error::Internal {
                source: format!("entrypoint {entrypoint:?} missing after validation").into(),
            })
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
        let bytes = self.entrypoint(&bundle).await?;

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

        let id = InstanceId::new(format!(
            "wasm-{}",
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        self.instances.lock().await.insert(
            id.clone(),
            Instance {
                bundle,
                logs: Arc::new(InstanceLogs::default()),
                stopped: false,
            },
        );
        Ok(Handle::new(ctx, id))
    }

    async fn invoke(
        &self,
        handle: &Handle,
        req: Request,
        opts: InvokeOptions,
    ) -> Result<Response> {
        let (bundle, logs) = {
            let instances = self.instances.lock().await;
            let instance = instances
                .get(handle.instance())
                .filter(|i| !i.stopped)
                .ok_or(Error::Stopped {})?;
            (instance.bundle, Arc::clone(&instance.logs))
        };

        let bytes = self.entrypoint(&bundle).await?;
        let component = self.cache.get_or_compile(&bundle, &bytes).await?;

        let call = invoke::run(
            self.cache.engine(),
            &component,
            handle.identity().clone(),
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
        let instances = self.instances.lock().await;
        let instance = instances
            .get(handle.instance())
            .filter(|i| !i.stopped)
            .ok_or(Error::Stopped {})?;
        Ok(LogStream::new(instance.logs.subscribe()))
    }

    async fn stop(&self, handle: Handle) -> Result<()> {
        if let Some(instance) = self.instances.lock().await.get_mut(handle.instance()) {
            instance.stopped = true;
        }
        Ok(())
    }
}

