//! `clwd run` — run an app locally in a sandbox.
//!
//! # What this accepts, and why not a bare `.wasm`
//!
//! A bundle, and only a bundle. The issue text said `clwd run ./app.wasm`, but
//! that predates the bundle becoming the unit of execution: `Sandbox::spawn`
//! takes a `BundleRef`, and everything that constrains an app —
//! `capabilities.limits.cpu_ms`, `memory_mb`, the secrets it declares, which
//! sandbox it targets — lives in the manifest a bare component does not have.
//!
//! Accepting one would mean either inventing default limits here, which is the
//! second source of truth for capabilities that the bundle format exists to
//! prevent, or running with none — a local posture so different from production
//! that an app working on a laptop could be killed in production by a limit its
//! author never saw. Refusing is the honest answer, and `clwd build` is one
//! command away.
//!
//! # The local posture
//!
//! Every store is in-memory and dies with the process. That is not a
//! placeholder: it is what makes `DigestAsAppId` safe to use here, and the two
//! choices must move together — see the note at the construction site.

use std::path::Path;
use std::sync::Arc;

use clwd_bundle::{BundleRef, Runtime};
use clwd_runtime::wasm::{
    AppDataStore, DigestAsAppId, MemoryAppDataStore, MemoryBundleStore, MemorySecretStore,
    SecretStore, WasmSandbox,
};
use clwd_runtime::{
    Body, Caller, IdentityCtx, InvokeOptions, OrgId, Principal, Sandbox, SpawnOptions, SubjectId,
};

use crate::cli::Backend;
use crate::error::CliError;
use crate::Progress;

mod output;
pub use output::Outcome;

/// The subject a local run presents.
///
/// Not anonymous. #29's templates will show app authors reading
/// `ctx().caller()`, and against an anonymous caller they would learn that the
/// anonymous branch is the normal case — when in production it is the
/// share-link case. A local run is a signed-in user; it is just obviously not a
/// real one.
///
/// Not configurable either. An `--as` flag invites people to build local auth
/// flows that do not exist in production.
const LOCAL_SUBJECT: &str = "local";

/// The org a local run presents.
///
/// Production orgs are slugs the control plane mints, and `local` is on the
/// platform's reserved list precisely so it can never be one. An app reading
/// `caller().org` gets `local` on a laptop and a real org in production —
/// the same code path, an obviously different value.
const LOCAL_ORG: &str = "local";

/// Run `bundle`, send one request to `path`, and return what came back.
pub fn execute(
    bundle: &Path,
    path: &str,
    backend: Option<Backend>,
    progress: Option<&dyn Progress>,
) -> Result<Outcome, CliError> {
    let bytes = read_bundle(bundle)?;

    // Decode before spawning, for the backend discriminant and to fail on a
    // damaged bundle here rather than inside the sandbox. The digest is not
    // passed as `expected`: nobody has told us what this bundle *should* be, so
    // there is nothing to check it against, and inventing one would turn every
    // run into a tautology.
    let decoded = clwd_bundle::decode(&bytes, None).map_err(from_bundle)?;

    // The manifest decides the backend; `--backend` overrides it. A bundle built
    // for microvm silently running on wasm would be worse than refusing.
    let target = match backend {
        Some(Backend::Wasm) => Runtime::Wasm,
        Some(Backend::MicroVm) => Runtime::MicroVm,
        None => decoded.manifest.runtime,
    };
    if target == Runtime::MicroVm {
        return Err(CliError::NotImplemented {
            command: "run --backend microvm",
        });
    }

    if let Some(p) = progress {
        p.step("starting the sandbox");
    }

    let digest = BundleRef::of_stream(&bytes);
    let sandbox = local_sandbox(&digest, bytes)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Internal { source: e.into() })?;

    runtime.block_on(async move {
        let handle = sandbox
            .spawn(digest, local_identity(), SpawnOptions::default())
            .await
            .map_err(from_runtime)?;

        if let Some(p) = progress {
            p.step("sending the request");
        }

        let request = http::Request::builder()
            .method("GET")
            .uri(format!("http://localhost{path}"))
            .body(Body::empty())
            .map_err(|e| CliError::InvalidArgument {
                what: format!("{path:?} is not a usable request path: {e}"),
            })?;

        let response = sandbox
            .invoke(&handle, request, InvokeOptions::default())
            .await
            .map_err(from_runtime)?;

        // Stopping is not `?`. The response is the answer, and failing to tear
        // down a sandbox in a process about to exit should not turn a successful
        // run into a failure.
        //
        // But discarding it entirely would make a genuine teardown failure
        // invisible, which is the wrong trade for whoever is later debugging a
        // leaked sandbox. Silent in the exit code, visible in the log.
        if let Err(e) = sandbox.stop(handle).await {
            tracing::debug!(error = %e, code = e.code(), "stopping the sandbox failed");
        }

        Ok(Outcome::new(response, LOCAL_SUBJECT, LOCAL_ORG))
    })
}

/// The caller a local run presents. See `LOCAL_SUBJECT`.
fn local_identity() -> IdentityCtx {
    IdentityCtx::new(Caller::User(Principal::new(
        SubjectId::new(LOCAL_SUBJECT),
        OrgId::new(LOCAL_ORG),
    )))
}

/// A sandbox whose every store lives in this process and dies with it.
fn local_sandbox(digest: &BundleRef, bytes: Vec<u8>) -> Result<WasmSandbox, CliError> {
    let bundles = MemoryBundleStore::new();
    bundles.insert(digest, bytes);

    // Three separate facts about this construction, none of them obvious.
    //
    // 1. `DigestAsAppId` and `MemoryAppDataStore` must change together. A digest
    //    changes on every build, so a *persistent* store keyed this way would
    //    move the app to an empty namespace on every rebuild and orphan whatever
    //    its users had stored — silently, looking like the app forgot. If this
    //    store ever gains a disk, the resolver gains a real app identity in the
    //    same commit.
    //
    // 2. Secrets are empty until #27 wires the OS keyring. Not a placeholder to
    //    be filled in; an empty store is the correct local posture today.
    //
    // 3. Empty is indistinguishable from ungranted, by design. The WIT makes
    //    `secrets.get` return `none` for both "no such secret" and "not granted
    //    to this app", so a guest cannot probe the difference — locally or in
    //    production.
    WasmSandbox::with_stores(
        Arc::new(bundles),
        Arc::new(MemorySecretStore::new()) as Arc<dyn SecretStore>,
        Arc::new(MemoryAppDataStore::new()) as Arc<dyn AppDataStore>,
        Arc::new(DigestAsAppId),
    )
    .map_err(from_runtime)
}

fn read_bundle(path: &Path) -> Result<Vec<u8>, CliError> {
    if path.extension().is_some_and(|e| e == "wasm") {
        return Err(CliError::InvalidArgument {
            what: format!(
                "{} is a component, not a bundle. `clwd run` needs the manifest that \
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
fn from_runtime(source: clwd_runtime::Error) -> CliError {
    CliError::Runtime { source }
}

/// Surface a bundle failure with its own code intact.
///
/// Direct rather than wrapped in `clwd_runtime::Error`: since #28 `CliError` has
/// its own `Bundle` variant, so there is no reason to route a failure the bundle
/// format decided through a runtime that had nothing to do with it.
fn from_bundle(source: clwd_bundle::Error) -> CliError {
    CliError::Bundle { source }
}
