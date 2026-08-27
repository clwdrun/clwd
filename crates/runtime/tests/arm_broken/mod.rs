//! SCRATCH — Phase 5 broken-arm experiment. Reverted after the run.
//!
//! Three `BackendUnderTest` impls that each wrap a real `WasmSandbox` and
//! corrupt exactly one thing a plausible-but-wrong backend would get wrong.
//! Selected by the `CONFORMANCE_ARM` env var (see `arms()` in `conformance.rs`).
//! Each should turn a specific, predicted set of requirements red and leave the
//! rest green — that is the evidence the suite can fail, and that each
//! requirement tests what its name claims.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clwd_bundle::{build, BundleRef, Capabilities, File, Limits, Runtime};
use clwd_runtime::conformance::{BackendUnderTest, FixtureKind, InstanceObservation, WorldSpec};
use clwd_runtime::handle::InstanceId;
use clwd_runtime::identity::IdentityCtx;
use clwd_runtime::wasm::{
    AppDataStore, AppId, AppIdResolver, BundleStore, DigestAsAppId, MemoryAppDataStore,
    MemoryBundleStore, MemorySecretStore, SecretStore, WasmSandbox,
};
use clwd_runtime::{
    Error, Handle, InvokeOptions, LogStream, Request, Response, Result, Sandbox, SpawnOptions,
};

const GUEST: &[u8] = include_bytes!("../fixtures/app.wasm");

type Bundle = (BundleRef, Vec<u8>);

/// Everything a real backend needs: the sandbox plus the three fixture bundles,
/// seeded into its store.
struct Real {
    sandbox: Arc<WasmSandbox>,
    guest: Bundle,
    ingest_failure: Bundle,
    tampered: Bundle,
}

fn guest_bundle(world: &WorldSpec) -> Bundle {
    let encoded = build(
        Runtime::Wasm,
        "app.wasm",
        Capabilities {
            version: 1,
            egress: Vec::new(),
            secrets: world.declared_secrets.clone(),
            storage_namespaces: Vec::new(),
            limits: Limits {
                cpu_ms: world.cpu_ms,
                memory_mb: world.memory_mb,
            },
        },
        vec![File {
            path: "app.wasm".to_owned(),
            data: GUEST.to_vec(),
            executable: false,
        }],
    )
    .expect("valid bundle");
    (encoded.digest, encoded.bytes)
}

fn ingest_failure_bundle() -> Bundle {
    let bytes = b"this is emphatically not a tar archive-".repeat(64);
    (BundleRef::of_stream(&bytes), bytes)
}

fn tampered_bundle() -> Bundle {
    let (_, mut bytes) = guest_bundle(&WorldSpec::defaults());
    bytes[512] ^= 0xff;
    (BundleRef::of_stream(&bytes), bytes)
}

fn real(world: &WorldSpec) -> Real {
    let guest = guest_bundle(world);
    let ingest_failure = ingest_failure_bundle();
    let tampered = tampered_bundle();

    let bundles = MemoryBundleStore::new();
    for (digest, bytes) in [&guest, &ingest_failure, &tampered] {
        bundles.insert(digest, bytes.clone());
    }
    let secrets = MemorySecretStore::new();
    let app = AppId::new(guest.0.to_string());
    for (name, value) in &world.store_secrets {
        secrets.insert(&app, name, value.clone());
    }
    let sandbox = Arc::new(
        WasmSandbox::with_stores(
            Arc::new(bundles) as Arc<dyn BundleStore>,
            Arc::new(secrets) as Arc<dyn SecretStore>,
            Arc::new(MemoryAppDataStore::new()) as Arc<dyn AppDataStore>,
            Arc::new(DigestAsAppId) as Arc<dyn AppIdResolver>,
        )
        .expect("engine builds"),
    );
    Real {
        sandbox,
        guest,
        ingest_failure,
        tampered,
    }
}

/// Shared arm scaffolding: stashes the last-built `Real` and forwards the
/// `BackendUnderTest` bookkeeping methods. The three arms differ only in which
/// `Sandbox` decorator `sandbox()` returns.
struct ArmState {
    built: Mutex<Option<Real>>,
    seen: Mutex<Vec<InstanceObservation>>,
}

impl ArmState {
    fn new() -> Self {
        Self {
            built: Mutex::new(None),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn set(&self, real: Real) {
        *self.built.lock().unwrap() = Some(real);
    }

    fn sandbox_arc(&self) -> Arc<WasmSandbox> {
        Arc::clone(
            &self
                .built
                .lock()
                .unwrap()
                .as_ref()
                .expect("sandbox() first")
                .sandbox,
        )
    }

    fn fixture(&self, kind: FixtureKind) -> Bundle {
        let guard = self.built.lock().unwrap();
        let r = guard.as_ref().expect("sandbox() first");
        match kind {
            FixtureKind::Guest => r.guest.clone(),
            FixtureKind::IngestFailure => r.ingest_failure.clone(),
            FixtureKind::Tampered => r.tampered.clone(),
            other => panic!("unknown FixtureKind {other:?}"),
        }
    }

    fn instances(&self) -> Vec<InstanceObservation> {
        self.seen.lock().unwrap().clone()
    }
}

// ======================================================================
// Arm 1: identity resolved from a bundle-keyed map, not from the handle.
// Predicted red: R1.5, R1.6. Everything else green.
// ======================================================================

pub struct IdentityByBundle {
    state: Arc<ArmState>,
}

impl IdentityByBundle {
    pub fn new() -> Self {
        Self {
            state: Arc::new(ArmState::new()),
        }
    }
}

#[async_trait]
impl BackendUnderTest for IdentityByBundle {
    fn name(&self) -> &'static str {
        "broken:identity-by-bundle"
    }
    fn available(&self) -> bool {
        true
    }
    async fn sandbox(&self, world: &WorldSpec) -> Box<dyn Sandbox> {
        let real = real(world);
        let inner = Arc::clone(&real.sandbox);
        self.state.set(real);
        Box::new(IbbSandbox {
            inner,
            seen: Arc::clone(&self.state),
            latest: Mutex::new(HashMap::new()),
            bundle_of: Mutex::new(HashMap::new()),
        })
    }
    fn fixture_bundle(&self, kind: FixtureKind) -> Bundle {
        self.state.fixture(kind)
    }
    fn instances(&self) -> Vec<InstanceObservation> {
        self.state.instances()
    }
    async fn evict(&self, id: &InstanceId) {
        self.state.sandbox_arc().evict(id).await;
    }
}

struct IbbSandbox {
    inner: Arc<WasmSandbox>,
    seen: Arc<ArmState>,
    /// bundle -> (most-recently-spawned instance id, the identity it was spawned with)
    latest: Mutex<HashMap<BundleRef, (InstanceId, IdentityCtx)>>,
    bundle_of: Mutex<HashMap<InstanceId, BundleRef>>,
}

#[async_trait]
impl Sandbox for IbbSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        let caller = ctx.caller.clone();
        let handle = self.inner.spawn(bundle, ctx.clone(), opts).await?;
        let id = handle.instance().clone();
        self.bundle_of.lock().unwrap().insert(id.clone(), bundle);
        self.latest
            .lock()
            .unwrap()
            .insert(bundle, (id.clone(), ctx));
        self.seen.seen.lock().unwrap().push(InstanceObservation {
            instance: id,
            callers: vec![caller],
        });
        Ok(handle)
    }
    async fn invoke(&self, handle: &Handle, req: Request, opts: InvokeOptions) -> Result<Response> {
        let bundle = *self
            .bundle_of
            .lock()
            .unwrap()
            .get(handle.instance())
            .expect("spawned here");
        let (target_id, target_ctx) = self.latest.lock().unwrap().get(&bundle).cloned().unwrap();
        // Route to whichever instance was spawned last for this bundle, as that
        // identity — the handle passed in is ignored.
        let fabricated = Handle::new(target_ctx, target_id);
        self.inner.invoke(&fabricated, req, opts).await
    }
    async fn logs_stream(&self, handle: &Handle) -> Result<LogStream> {
        self.inner.logs_stream(handle).await
    }
    async fn stop(&self, handle: Handle) -> Result<()> {
        self.inner.stop(handle).await
    }
}

// ======================================================================
// Arm 2: bundle.* errors from spawn re-spelled as a runtime.* code.
// Predicted red: R2.4. Everything else green.
// ======================================================================

pub struct BundleCodeRemap {
    state: Arc<ArmState>,
}

impl BundleCodeRemap {
    pub fn new() -> Self {
        Self {
            state: Arc::new(ArmState::new()),
        }
    }
}

#[async_trait]
impl BackendUnderTest for BundleCodeRemap {
    fn name(&self) -> &'static str {
        "broken:bundle-code-remap"
    }
    fn available(&self) -> bool {
        true
    }
    async fn sandbox(&self, world: &WorldSpec) -> Box<dyn Sandbox> {
        let real = real(world);
        let inner = Arc::clone(&real.sandbox);
        self.state.set(real);
        Box::new(RemapSandbox { inner })
    }
    fn fixture_bundle(&self, kind: FixtureKind) -> Bundle {
        self.state.fixture(kind)
    }
    fn instances(&self) -> Vec<InstanceObservation> {
        self.state.instances()
    }
    async fn evict(&self, id: &InstanceId) {
        self.state.sandbox_arc().evict(id).await;
    }
}

struct RemapSandbox {
    inner: Arc<WasmSandbox>,
}

#[async_trait]
impl Sandbox for RemapSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        self.inner.spawn(bundle, ctx, opts).await.map_err(|e| {
            if e.code().starts_with("bundle.") {
                Error::Internal {
                    source: format!("bundle rejected ({})", e.code()).into(),
                }
            } else {
                e
            }
        })
    }
    async fn invoke(&self, handle: &Handle, req: Request, opts: InvokeOptions) -> Result<Response> {
        self.inner.invoke(handle, req, opts).await
    }
    async fn logs_stream(&self, handle: &Handle) -> Result<LogStream> {
        self.inner.logs_stream(handle).await
    }
    async fn stop(&self, handle: Handle) -> Result<()> {
        self.inner.stop(handle).await
    }
}

// ======================================================================
// Arm 3: an invoke-phase deadline reported as a trap.
// Predicted red: R2.6, R4.1, R4.2, R4.3. R3.1 (a real trap) stays green.
// ======================================================================

pub struct TrapForInvokeDeadline {
    state: Arc<ArmState>,
}

impl TrapForInvokeDeadline {
    pub fn new() -> Self {
        Self {
            state: Arc::new(ArmState::new()),
        }
    }
}

#[async_trait]
impl BackendUnderTest for TrapForInvokeDeadline {
    fn name(&self) -> &'static str {
        "broken:trap-for-deadline"
    }
    fn available(&self) -> bool {
        true
    }
    async fn sandbox(&self, world: &WorldSpec) -> Box<dyn Sandbox> {
        let real = real(world);
        let inner = Arc::clone(&real.sandbox);
        self.state.set(real);
        Box::new(TrapSandbox { inner })
    }
    fn fixture_bundle(&self, kind: FixtureKind) -> Bundle {
        self.state.fixture(kind)
    }
    fn instances(&self) -> Vec<InstanceObservation> {
        self.state.instances()
    }
    async fn evict(&self, id: &InstanceId) {
        self.state.sandbox_arc().evict(id).await;
    }
}

struct TrapSandbox {
    inner: Arc<WasmSandbox>,
}

#[async_trait]
impl Sandbox for TrapSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        self.inner.spawn(bundle, ctx, opts).await
    }
    async fn invoke(&self, handle: &Handle, req: Request, opts: InvokeOptions) -> Result<Response> {
        self.inner
            .invoke(handle, req, opts)
            .await
            .map_err(|e| match e {
                Error::DeadlineExceeded {
                    phase: clwd_runtime::Phase::Invoke,
                    ..
                } => Error::Trap {
                    message: "guest exceeded its time".into(),
                },
                other => other,
            })
    }
    async fn logs_stream(&self, handle: &Handle) -> Result<LogStream> {
        self.inner.logs_stream(handle).await
    }
    async fn stop(&self, handle: Handle) -> Result<()> {
        self.inner.stop(handle).await
    }
}
