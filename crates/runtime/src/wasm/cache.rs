//! Compiled-component cache.
//!
//! Compilation costs ~90ms for a small component, so a cache is not an
//! optimization here — it is the difference between a warm spawn and a cold
//! one on every request.

use std::collections::HashMap;
use std::sync::Arc;

use clwd_bundle::BundleRef;
use tokio::sync::{Mutex, OnceCell};
use wasmtime::component::{Component, Linker};
use wasmtime::Engine;
use wasmtime_wasi_http::p2::bindings::ProxyPre;

use crate::error::Error;
use crate::wasm::host::StoreState;

/// A component pre-instantiated against the linker: the per-instantiation work
/// that can be done once. Measured at ~78µs, comparable to instantiation
/// itself, so it is cached rather than repeated per request.
pub(super) type Prepared = ProxyPre<StoreState>;

/// Default budget: enough for a working set of small components without
/// letting a busy host accumulate compiled code without bound.
const DEFAULT_BUDGET_BYTES: usize = 256 * 1024 * 1024;

struct Entry {
    prepared: Prepared,
    /// Serialized size, measured once at insert. Components vary in how much
    /// machine code they compile to — the probe's 106KB component serialized to
    /// 331KB, roughly 3x, but that ratio is not a constant. An entry-count cap
    /// would therefore be an unbounded memory budget, which is why this is
    /// bytes.
    size: usize,
    /// Monotonic tick of last use, for LRU eviction.
    used: u64,
}

/// Compiled components, keyed by the digest of the bundle they came from.
///
/// # The key is the bundle digest alone, and that is only correct in memory
///
/// A compiled `Component` cannot outlive the `Engine` it was compiled for, and
/// this cache is owned by the `WasmSandbox` that owns that `Engine`. The
/// wasmtime version and the `Config` are therefore pinned by construction
/// rather than by the key.
///
/// **The moment this is persisted to disk that stops being true**, and the key
/// must gain `wasmtime::VERSION` and a fingerprint of the `Config` — otherwise
/// an artifact compiled by another version or another configuration is loaded
/// against an engine that cannot run it. `Component::deserialize` does check a
/// compatibility hash and will refuse, so the failure is loud rather than
/// silent, but a cache that misses on every entry after an upgrade is its own
/// kind of outage.
pub(super) struct ComponentCache {
    engine: Engine,
    linker: Linker<StoreState>,
    budget: usize,
    state: Mutex<State>,
    /// Counts actual compilations, so a test can tell single-flight working
    /// from two threads compiling the same bundle and discarding one result.
    compilations: std::sync::atomic::AtomicU64,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    /// In-flight compilations, so two spawns of one uncached bundle compile
    /// once. The cell is cloned out and awaited *after* the lock is released:
    /// holding a lock across a ~90ms compile would serialize every spawn in the
    /// process, and not sharing the cell would duplicate the work.
    in_flight: HashMap<String, Arc<OnceCell<(Prepared, usize)>>>,
    bytes: usize,
    tick: u64,
}

impl ComponentCache {
    pub(super) fn new(engine: Engine) -> Result<Self, Error> {
        let mut linker: Linker<StoreState> = Linker::new(&engine);
        let wire = |e: wasmtime::Error| Error::Internal {
            source: format!("linker: {e}").into(),
        };
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(wire)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker).map_err(wire)?;
        crate::wasm::host::add_identity_to_linker(&mut linker).map_err(wire)?;

        Ok(Self {
            engine,
            linker,
            budget: DEFAULT_BUDGET_BYTES,
            state: Mutex::new(State::default()),
            compilations: std::sync::atomic::AtomicU64::new(0),
        })
    }

    pub(super) fn engine(&self) -> &Engine {
        &self.engine
    }

    /// How many entries are resident. Tests assert on this rather than on
    /// timing, which would be a proxy for the thing they mean.
    pub(super) async fn len(&self) -> usize {
        self.state.lock().await.entries.len()
    }

    /// The resident component for `bundle`, if there is one.
    ///
    /// Lets `invoke` skip resolving bytes entirely on the common path.
    /// Resolving means decoding the bundle, which re-verifies every member's
    /// BLAKE3 — work proportional to bundle size, on every request, for an
    /// artifact that was already validated at spawn.
    pub(super) async fn get(&self, bundle: &BundleRef) -> Option<Prepared> {
        let mut state = self.state.lock().await;
        state.tick += 1;
        let tick = state.tick;
        let entry = state.entries.get_mut(&bundle.to_string())?;
        entry.used = tick;
        Some(entry.prepared.clone())
    }

    pub(super) fn compilations(&self) -> u64 {
        self.compilations.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Fetch the compiled component for `bundle`, compiling `bytes` if it is
    /// not resident.
    pub(super) async fn get_or_compile(
        &self,
        bundle: &BundleRef,
        bytes: &[u8],
    ) -> Result<Prepared, Error> {
        let key = bundle.to_string();

        // Fast path: resident.
        {
            let mut state = self.state.lock().await;
            state.tick += 1;
            let tick = state.tick;
            if let Some(entry) = state.entries.get_mut(&key) {
                entry.used = tick;
                return Ok(entry.prepared.clone());
            }
        }

        // Slow path: single-flight. Take or create the cell under a short lock,
        // then release it before awaiting.
        let cell = {
            let mut state = self.state.lock().await;
            if let Some(entry) = state.entries.get(&key) {
                return Ok(entry.prepared.clone());
            }
            Arc::clone(state.in_flight.entry(key.clone()).or_default())
        };

        let engine = self.engine.clone();
        let owned = bytes.to_vec();
        let linker = self.linker.clone();
        let counter = &self.compilations;
        let prepared = cell
            .get_or_try_init(|| async move {
                // Compilation is CPU-bound and takes ~90ms. On the async
                // executor that would stall every other task on the thread.
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let component =
                    tokio::task::spawn_blocking(move || Component::new(&engine, &owned))
                        .await
                        .map_err(|e| Error::Internal {
                            source: format!("compilation task: {e}").into(),
                        })?
                        // A core module reaches here only if it got past
                        // `clwd-bundle`'s ingest check; wasmtime rejects it too,
                        // because the component preamble is what it parses for.
                        .map_err(|e| Error::Internal {
                            source: format!("compiling component: {e}").into(),
                        })?;
                let size = component.serialize().map(|b| b.len()).unwrap_or(0);
                let pre = linker
                    .instantiate_pre(&component)
                    .map_err(|e| Error::Internal {
                        source: format!("pre-instantiating: {e}").into(),
                    })?;
                let prepared = ProxyPre::new(pre).map_err(|e| Error::Internal {
                    source: format!("component does not export wasi:http/proxy: {e}").into(),
                })?;
                Ok::<_, Error>((prepared, size))
            })
            .await?
            .clone();

        self.insert(key, prepared.clone()).await;
        Ok(prepared.0)
    }

    async fn insert(&self, key: String, entry: (Prepared, usize)) {
        let (prepared, size) = entry;
        let mut state = self.state.lock().await;
        state.in_flight.remove(&key);
        state.tick += 1;
        let used = state.tick;

        if state.entries.contains_key(&key) {
            return;
        }
        state.bytes += size;
        state.entries.insert(
            key,
            Entry {
                prepared,
                size,
                used,
            },
        );

        // Evict least-recently-used until under budget. Linear, because the
        // resident set is dozens of entries and a linked list would be more
        // machinery than the scan costs.
        while state.bytes > self.budget && state.entries.len() > 1 {
            let Some(victim) = state
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(entry) = state.entries.remove(&victim) {
                state.bytes -= entry.size;
            }
        }
    }
}
