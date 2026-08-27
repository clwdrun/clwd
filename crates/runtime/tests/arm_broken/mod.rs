//! SCRATCH — Phase 5 broken-arm experiment (round 2). Reverted after the run.
//!
//! Four `BackendUnderTest` impls, each wrapping a real `WasmSandbox` and
//! corrupting one thing a plausible-but-wrong backend would get wrong.
//! Selected by `CONFORMANCE_ARM` (see `arms()` in `conformance.rs`).
//!
//! - identity-by-bundle: identity resolved from the last spawn for a bundle,
//!   not from the handle. Confirms the fixed R1.1 now catches invoke-time
//!   crossing, alongside R1.5/R1.6.
//! - trap-for-deadline: an invoke-phase deadline reported as a trap. Confirms
//!   the fixed R4.5/R4.6 now catch a mislabelled preemption.
//! - secrets-ignore-grant: the broker returns everything the store holds,
//!   ignoring the manifest. Should red R5.2.
//! - appdata-per-caller: the app namespace is keyed by caller, not by app.
//!   Should red R6.3 and R6.4.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clwd_bundle::{build, BundleRef, Capabilities, File, Limits, Runtime};
use clwd_runtime::conformance::{BackendUnderTest, FixtureKind, InstanceObservation, WorldSpec};
use clwd_runtime::handle::InstanceId;
use clwd_runtime::identity::{Caller, IdentityCtx};
use clwd_runtime::wasm::{
    AppDataStore, AppId, AppIdResolver, BundleStore, DigestAsAppId, MemoryAppDataStore,
    MemoryBundleStore, MemorySecretStore, SecretStore, WasmSandbox,
};
use clwd_runtime::{
    Error, Handle, InvokeOptions, LogStream, Request, Response, Result, Sandbox, SpawnOptions,
};

const GUEST: &[u8] = include_bytes!("../fixtures/app.wasm");
type Bundle = (BundleRef, Vec<u8>);
type Recorder = Arc<Mutex<Vec<InstanceObservation>>>;

fn subject_of(caller: &Caller) -> String {
    match caller {
        Caller::Anonymous => "anonymous".into(),
        Caller::User(p) => p.subject.to_string(),
        Caller::Service(s) => s.subject.to_string(),
    }
}

fn guest_bundle(world: &WorldSpec, marker: Option<&str>) -> Bundle {
    let mut secrets = world.declared_secrets.clone();
    secrets.extend(marker.map(str::to_owned));
    let encoded = build(
        Runtime::Wasm,
        "app.wasm",
        Capabilities {
            version: 1,
            egress: Vec::new(),
            secrets,
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

fn broken_bundles() -> (Bundle, Bundle) {
    let ingest = {
        let bytes = b"this is emphatically not a tar archive-".repeat(64);
        (BundleRef::of_stream(&bytes), bytes)
    };
    let tampered = {
        let (_, mut bytes) = guest_bundle(&WorldSpec::defaults(), None);
        bytes[512] ^= 0xff;
        (BundleRef::of_stream(&bytes), bytes)
    };
    (ingest, tampered)
}

/// The fixture bundles, common to every arm. `assemble` seeds them into a store.
struct Parts {
    guest: Bundle,
    guest2: Bundle,
    ingest_failure: Bundle,
    tampered: Bundle,
    app: AppId,
    store_secrets: Vec<(String, Vec<u8>)>,
}

fn parts(world: &WorldSpec) -> Parts {
    let guest = guest_bundle(world, None);
    let guest2 = guest_bundle(world, Some("_guest2_marker"));
    let (ingest_failure, tampered) = broken_bundles();
    Parts {
        app: AppId::new(guest.0.to_string()),
        store_secrets: world.store_secrets.clone(),
        guest,
        guest2,
        ingest_failure,
        tampered,
    }
}

/// Shared arm bookkeeping. Each arm stashes its `WasmSandbox` and the fixture
/// bundles here; only the `Sandbox` decorator differs.
struct Stash {
    sandbox: Mutex<Option<Arc<WasmSandbox>>>,
    seen: Recorder,
    bundles: Mutex<Option<(Bundle, Bundle, Bundle, Bundle)>>,
}

impl Stash {
    fn new() -> Self {
        Self {
            sandbox: Mutex::new(None),
            seen: Arc::new(Mutex::new(Vec::new())),
            bundles: Mutex::new(None),
        }
    }

    fn set(&self, sb: Arc<WasmSandbox>, p: &Parts) {
        *self.sandbox.lock().unwrap() = Some(sb);
        *self.bundles.lock().unwrap() = Some((
            p.guest.clone(),
            p.guest2.clone(),
            p.ingest_failure.clone(),
            p.tampered.clone(),
        ));
    }

    fn record(&self, handle: &Handle, ctx: &IdentityCtx) {
        self.seen.lock().unwrap().push(InstanceObservation {
            instance: handle.instance().clone(),
            callers: vec![ctx.caller.clone()],
        });
    }

    fn fixture(&self, kind: FixtureKind) -> Bundle {
        let guard = self.bundles.lock().unwrap();
        let (g, g2, ing, tam) = guard.as_ref().expect("sandbox() first");
        match kind {
            FixtureKind::Guest => g.clone(),
            FixtureKind::Guest2 => g2.clone(),
            FixtureKind::IngestFailure => ing.clone(),
            FixtureKind::Tampered => tam.clone(),
            other => panic!("unknown FixtureKind {other:?}"),
        }
    }

    fn instances(&self) -> Vec<InstanceObservation> {
        self.seen.lock().unwrap().clone()
    }

    async fn evict(&self, id: &InstanceId) {
        let sb = self
            .sandbox
            .lock()
            .unwrap()
            .clone()
            .expect("sandbox() first");
        sb.evict(id).await;
    }
}

fn seed_secrets(store: &MemorySecretStore, app: &AppId, pairs: &[(String, Vec<u8>)]) {
    for (name, value) in pairs {
        store.insert(app, name, value.clone());
    }
}

/// Assemble a `WasmSandbox` from parts with the given secret store and resolver.
fn assemble(
    p: &Parts,
    secrets: Arc<dyn SecretStore>,
    resolver: Arc<dyn AppIdResolver>,
) -> Arc<WasmSandbox> {
    let store = MemoryBundleStore::new();
    for pair in [&p.guest, &p.guest2, &p.ingest_failure, &p.tampered] {
        store.insert(&pair.0, pair.1.clone());
    }
    Arc::new(
        WasmSandbox::with_stores(
            Arc::new(store) as Arc<dyn BundleStore>,
            secrets,
            Arc::new(MemoryAppDataStore::new()) as Arc<dyn AppDataStore>,
            resolver,
        )
        .expect("engine builds"),
    )
}

// ======================================================================
// Arm 1: identity from the last spawn for a bundle, not from the handle.
// Predicted red: R1.1, R1.5, R1.6.
// ======================================================================

pub struct IdentityByBundle {
    stash: Arc<Stash>,
}
impl IdentityByBundle {
    pub fn new() -> Self {
        Self {
            stash: Arc::new(Stash::new()),
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
        let p = parts(world);
        let secrets = MemorySecretStore::new();
        seed_secrets(&secrets, &p.app, &p.store_secrets);
        let sb = assemble(
            &p,
            Arc::new(secrets),
            Arc::new(DigestAsAppId) as Arc<dyn AppIdResolver>,
        );
        self.stash.set(Arc::clone(&sb), &p);
        Box::new(IbbSandbox {
            inner: sb,
            stash: Arc::clone(&self.stash),
            latest: Mutex::new(HashMap::new()),
            bundle_of: Mutex::new(HashMap::new()),
        })
    }
    fn fixture_bundle(&self, kind: FixtureKind) -> Bundle {
        self.stash.fixture(kind)
    }
    fn instances(&self) -> Vec<InstanceObservation> {
        self.stash.instances()
    }
    async fn evict(&self, id: &InstanceId) {
        self.stash.evict(id).await;
    }
}

struct IbbSandbox {
    inner: Arc<WasmSandbox>,
    stash: Arc<Stash>,
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
        let handle = self.inner.spawn(bundle, ctx.clone(), opts).await?;
        let id = handle.instance().clone();
        self.bundle_of.lock().unwrap().insert(id.clone(), bundle);
        self.latest
            .lock()
            .unwrap()
            .insert(bundle, (id, ctx.clone()));
        self.stash.record(&handle, &ctx);
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
// Arm 2: an invoke-phase deadline reported as a trap.
// Predicted red: R2.6, R4.1, R4.2, R4.3, R4.5, R4.6. R3.1 stays green.
// ======================================================================

pub struct TrapForInvokeDeadline {
    stash: Arc<Stash>,
}
impl TrapForInvokeDeadline {
    pub fn new() -> Self {
        Self {
            stash: Arc::new(Stash::new()),
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
        let p = parts(world);
        let secrets = MemorySecretStore::new();
        seed_secrets(&secrets, &p.app, &p.store_secrets);
        let sb = assemble(
            &p,
            Arc::new(secrets),
            Arc::new(DigestAsAppId) as Arc<dyn AppIdResolver>,
        );
        self.stash.set(Arc::clone(&sb), &p);
        Box::new(TrapSandbox {
            inner: sb,
            stash: Arc::clone(&self.stash),
        })
    }
    fn fixture_bundle(&self, kind: FixtureKind) -> Bundle {
        self.stash.fixture(kind)
    }
    fn instances(&self) -> Vec<InstanceObservation> {
        self.stash.instances()
    }
    async fn evict(&self, id: &InstanceId) {
        self.stash.evict(id).await;
    }
}

struct TrapSandbox {
    inner: Arc<WasmSandbox>,
    stash: Arc<Stash>,
}

#[async_trait]
impl Sandbox for TrapSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        let handle = self.inner.spawn(bundle, ctx.clone(), opts).await?;
        self.stash.record(&handle, &ctx);
        Ok(handle)
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

// ======================================================================
// Arm 3: the secrets broker returns everything the store holds, ignoring
// the manifest. Predicted red: R5.2.
// ======================================================================

pub struct SecretsIgnoreGrant {
    stash: Arc<Stash>,
}
impl SecretsIgnoreGrant {
    pub fn new() -> Self {
        Self {
            stash: Arc::new(Stash::new()),
        }
    }
}

#[async_trait]
impl BackendUnderTest for SecretsIgnoreGrant {
    fn name(&self) -> &'static str {
        "broken:secrets-ignore-grant"
    }
    fn available(&self) -> bool {
        true
    }
    async fn sandbox(&self, world: &WorldSpec) -> Box<dyn Sandbox> {
        let p = parts(world);
        let secrets = Arc::new(IgnoreGrantStore {
            app: p.app.clone(),
            held: p.store_secrets.clone(),
        }) as Arc<dyn SecretStore>;
        let sb = assemble(
            &p,
            secrets,
            Arc::new(DigestAsAppId) as Arc<dyn AppIdResolver>,
        );
        self.stash.set(Arc::clone(&sb), &p);
        Box::new(PassthroughSandbox {
            inner: sb,
            stash: Arc::clone(&self.stash),
        })
    }
    fn fixture_bundle(&self, kind: FixtureKind) -> Bundle {
        self.stash.fixture(kind)
    }
    fn instances(&self) -> Vec<InstanceObservation> {
        self.stash.instances()
    }
    async fn evict(&self, id: &InstanceId) {
        self.stash.evict(id).await;
    }
}

struct IgnoreGrantStore {
    app: AppId,
    held: Vec<(String, Vec<u8>)>,
}

#[async_trait]
impl SecretStore for IgnoreGrantStore {
    async fn resolve(&self, app: &AppId, _names: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
        // The manifest's declared names are ignored: hand back everything held.
        Ok(if *app == self.app {
            self.held.clone()
        } else {
            Vec::new()
        })
    }
}

// ======================================================================
// Arm 4: the app namespace is keyed by the calling subject, not the app.
// Predicted red: R6.3, R6.4.
// ======================================================================

pub struct AppDataPerCaller {
    stash: Arc<Stash>,
}
impl AppDataPerCaller {
    pub fn new() -> Self {
        Self {
            stash: Arc::new(Stash::new()),
        }
    }
}

#[async_trait]
impl BackendUnderTest for AppDataPerCaller {
    fn name(&self) -> &'static str {
        "broken:appdata-per-caller"
    }
    fn available(&self) -> bool {
        true
    }
    async fn sandbox(&self, world: &WorldSpec) -> Box<dyn Sandbox> {
        let p = parts(world);
        let secrets = MemorySecretStore::new();
        seed_secrets(&secrets, &p.app, &p.store_secrets);
        let resolver = Arc::new(PerCallerResolver {
            pending: Mutex::new(None),
        });
        let sb = assemble(
            &p,
            Arc::new(secrets),
            Arc::clone(&resolver) as Arc<dyn AppIdResolver>,
        );
        self.stash.set(Arc::clone(&sb), &p);
        Box::new(PerCallerSandbox {
            inner: sb,
            stash: Arc::clone(&self.stash),
            resolver,
        })
    }
    fn fixture_bundle(&self, kind: FixtureKind) -> Bundle {
        self.stash.fixture(kind)
    }
    fn instances(&self) -> Vec<InstanceObservation> {
        self.stash.instances()
    }
    async fn evict(&self, id: &InstanceId) {
        self.stash.evict(id).await;
    }
}

/// Produces an `AppId` from the caller the decorator last announced, not from
/// the bundle. Spawns in the tests are sequential, so the side channel is safe.
struct PerCallerResolver {
    pending: Mutex<Option<Caller>>,
}

#[async_trait]
impl AppIdResolver for PerCallerResolver {
    async fn resolve(&self, _bundle: &BundleRef) -> Result<AppId> {
        let caller = self.pending.lock().unwrap().take();
        Ok(AppId::new(
            caller
                .map(|c| subject_of(&c))
                .unwrap_or_else(|| "unknown".into()),
        ))
    }
}

struct PerCallerSandbox {
    inner: Arc<WasmSandbox>,
    stash: Arc<Stash>,
    resolver: Arc<PerCallerResolver>,
}

#[async_trait]
impl Sandbox for PerCallerSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        *self.resolver.pending.lock().unwrap() = Some(ctx.caller.clone());
        let handle = self.inner.spawn(bundle, ctx.clone(), opts).await?;
        self.stash.record(&handle, &ctx);
        Ok(handle)
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

/// Forwards everything unchanged. Used by arms that corrupt via a store or
/// resolver rather than the `Sandbox` surface.
struct PassthroughSandbox {
    inner: Arc<WasmSandbox>,
    stash: Arc<Stash>,
}

#[async_trait]
impl Sandbox for PassthroughSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        let handle = self.inner.spawn(bundle, ctx.clone(), opts).await?;
        self.stash.record(&handle, &ctx);
        Ok(handle)
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
