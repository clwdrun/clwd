//! The `wasm` backend arm of the conformance suite.
//!
//! A [`RecordingSandbox`] wraps a real [`WasmSandbox`] and records the caller
//! each `spawn` was minted for — the generalised form of the builder-and-
//! recorder pattern from `test_util`. `WasmArm` also owns the fixture bundles
//! (`FIXTURE.md`'s guest plus two broken bundles for `R2.4`) and seeds them
//! into the sandbox's store when it is built.
//!
//! `sandbox()` is called at most once per `WasmArm` value; the suite builds a
//! fresh arm for every test. `fixture_bundle`/`instances`/`evict` read what
//! that call stashed and panic if it has not happened yet.

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
    Handle, InvokeOptions, LogStream, Request, Response, Result, Sandbox, SpawnOptions,
};

/// The reference guest component (`FIXTURE.md`). Rebuild with
/// `just rebuild-guest`.
const GUEST: &[u8] = include_bytes!("../fixtures/app.wasm");

/// A real bundle wrapping the reference guest, with the manifest shaped by
/// `world` (declared secrets, sealed limits).
fn guest_bundle(world: &WorldSpec) -> (BundleRef, Vec<u8>) {
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
    .expect("the reference guest is a valid bundle");
    (encoded.digest, encoded.bytes)
}

/// Bytes that are not a tar stream at all: `clwd-bundle` rejects them at the
/// ingest gate, so `spawn` surfaces a `bundle.*` code.
fn ingest_failure_bundle() -> (BundleRef, Vec<u8>) {
    let bytes = b"this is emphatically not a tar archive-".repeat(64);
    let digest = BundleRef::of_stream(&bytes);
    (digest, bytes)
}

/// A well-formed bundle with one member content byte flipped, so the member no
/// longer matches its declared digest.
fn tampered_bundle() -> (BundleRef, Vec<u8>) {
    let (_, mut bytes) = guest_bundle(&WorldSpec::defaults());
    bytes[512] ^= 0xff; // first content byte of the first member
    let digest = BundleRef::of_stream(&bytes);
    (digest, bytes)
}

/// What `sandbox()` produced, for `fixture_bundle`/`instances`/`evict` to read.
struct Built {
    sandbox: Arc<WasmSandbox>,
    seen: Arc<Mutex<Vec<InstanceObservation>>>,
    guest: (BundleRef, Vec<u8>),
    ingest_failure: (BundleRef, Vec<u8>),
    tampered: (BundleRef, Vec<u8>),
}

pub struct WasmArm {
    built: Mutex<Option<Built>>,
}

impl WasmArm {
    pub fn new() -> Self {
        Self {
            built: Mutex::new(None),
        }
    }

    fn get(&self) -> std::sync::MutexGuard<'_, Option<Built>> {
        self.built.lock().expect("not poisoned")
    }
}

#[async_trait]
impl BackendUnderTest for WasmArm {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn available(&self) -> bool {
        true
    }

    async fn sandbox(&self, world: &WorldSpec) -> Box<dyn Sandbox> {
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
            .expect("the wasm engine builds"),
        );

        let seen = Arc::new(Mutex::new(Vec::new()));
        *self.get() = Some(Built {
            sandbox: Arc::clone(&sandbox),
            seen: Arc::clone(&seen),
            guest,
            ingest_failure,
            tampered,
        });

        Box::new(RecordingSandbox {
            inner: sandbox,
            seen,
        })
    }

    fn fixture_bundle(&self, kind: FixtureKind) -> (BundleRef, Vec<u8>) {
        let guard = self.get();
        let built = guard
            .as_ref()
            .expect("call sandbox() before fixture_bundle()");
        match kind {
            FixtureKind::Guest => built.guest.clone(),
            FixtureKind::IngestFailure => built.ingest_failure.clone(),
            FixtureKind::Tampered => built.tampered.clone(),
            other => panic!("unknown FixtureKind {other:?}"),
        }
    }

    fn instances(&self) -> Vec<InstanceObservation> {
        let seen = {
            let guard = self.get();
            Arc::clone(
                &guard
                    .as_ref()
                    .expect("call sandbox() before instances()")
                    .seen,
            )
        };
        let observations = seen.lock().expect("not poisoned");
        observations.clone()
    }

    async fn evict(&self, id: &InstanceId) {
        let sandbox = {
            let guard = self.get();
            Arc::clone(
                &guard
                    .as_ref()
                    .expect("call sandbox() before evict()")
                    .sandbox,
            )
        };
        sandbox.evict(id).await;
    }
}

/// Forwards to a real [`WasmSandbox`], recording the caller each `spawn` was
/// minted for. `invoke` takes no identity, so there is no forwarding path
/// through which a second caller could be recorded against one instance — which
/// is what makes `R1.1` hold here by construction.
struct RecordingSandbox {
    inner: Arc<WasmSandbox>,
    seen: Arc<Mutex<Vec<InstanceObservation>>>,
}

#[async_trait]
impl Sandbox for RecordingSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        let caller = ctx.caller.clone();
        let handle = self.inner.spawn(bundle, ctx, opts).await?;
        self.seen
            .lock()
            .expect("not poisoned")
            .push(InstanceObservation {
                instance: handle.instance().clone(),
                callers: vec![caller],
            });
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
