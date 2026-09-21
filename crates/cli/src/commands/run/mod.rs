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
//! Every store is in-memory and dies with the process — see [`crate::commands::local`],
//! which `run` and `dev` share so the sandbox is built exactly one way.

use std::path::Path;

use clwd_bundle::{BundleRef, Runtime};
use clwd_runtime::{Body, InvokeOptions, Sandbox, SpawnOptions};

use crate::Progress;
use crate::cli::Backend;
use crate::commands::local;
use crate::error::CliError;

mod output;
pub use output::Outcome;

/// Run `bundle`, send one request to `path`, and return what came back.
pub fn execute(
    bundle: &Path,
    path: &str,
    backend: Option<Backend>,
    project: &Path,
    progress: Option<&dyn Progress>,
) -> Result<Outcome, CliError> {
    let bytes = local::read_bundle(bundle)?;

    // Decode before spawning, for the backend discriminant and to fail on a
    // damaged bundle here rather than inside the sandbox. The digest is not
    // passed as `expected`: nobody has told us what this bundle *should* be, so
    // there is nothing to check it against, and inventing one would turn every
    // run into a tautology.
    let decoded = clwd_bundle::decode(&bytes, None).map_err(local::from_bundle)?;

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
    let sandbox = local::local_sandbox(&digest, bytes, project)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::Internal { source: e.into() })?;

    runtime.block_on(async move {
        let handle = sandbox
            .spawn(digest, local::local_identity(), SpawnOptions::default())
            .await
            .map_err(local::from_runtime)?;

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
            .map_err(local::from_runtime)?;

        // Stopping is not `?`. The response is the answer, and failing to tear
        // down a sandbox in a process about to exit should not turn a successful
        // run into a failure. Silent in the exit code, visible in the log.
        if let Err(e) = sandbox.stop(handle).await {
            tracing::debug!(error = %e, code = e.code(), "stopping the sandbox failed");
        }

        Ok(Outcome::new(
            response,
            local::LOCAL_SUBJECT,
            local::LOCAL_ORG,
        ))
    })
}
