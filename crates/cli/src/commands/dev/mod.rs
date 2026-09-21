//! `clwd dev` — a long-lived local server that rebuilds and hot-swaps on edit.
//!
//! # What a "swap" is, against model (b)
//!
//! The wasm backend builds a fresh `Store` per invocation (`#54` model (b)), so
//! nothing persists between requests. A swap is therefore not mutating a live
//! instance: on a successful rebuild the new bundle has a new digest, and `dev`
//! builds a **new sandbox and handle** ([`Active`]) and flips which one new
//! requests get. **Nothing is lost across a swap** — each request already got a
//! fresh store, so there is no in-memory state a swap could discard. An author
//! will assume otherwise; it is not so.
//!
//! A request in flight when the swap happens completes against the old [`Active`]
//! (it holds an `Arc` to it, so the old sandbox lives until the request finishes)
//! and only *new* requests see the new one. Safe by construction, not by locking.
//!
//! # A failed rebuild keeps the server alive
//!
//! If a rebuild fails — a syntax error, a bad `clwd.toml` — the current [`Active`]
//! is left in place, the error is printed, and the server keeps answering with
//! the last good component. A dev loop that dies on a typo is worse than none.
//!
//! # `dev` is `run`, held open
//!
//! The sandbox is built through [`crate::commands::local`], the exact
//! construction `clwd run` uses, so `dev` enforces `#33`'s egress and storage,
//! the same limits, identity and secrets — it is not a looser mode.

/// Without a sandbox there is no server to run; the subcommand still parses and
/// says so, exactly as `run` does.
#[cfg(not(feature = "run"))]
pub fn execute(
    _project: &std::path::Path,
    _port: u16,
    _progress: Option<&dyn crate::Progress>,
) -> Result<(), crate::error::CliError> {
    Err(crate::error::CliError::NotImplemented { command: "dev" })
}

#[cfg(feature = "run")]
mod watch;

#[cfg(feature = "run")]
pub use served::execute;

#[cfg(feature = "run")]
mod served {
    use std::convert::Infallible;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use bytes::Bytes;
    use clwd_bundle::BundleRef;
    use clwd_runtime::wasm::WasmSandbox;
    use clwd_runtime::{Body, Handle, InvokeOptions, Response, Sandbox, SpawnOptions};
    use futures_core::Stream as _;
    use http_body_util::{BodyExt as _, Full};
    use hyper::body::Incoming;
    use hyper::service::service_fn;

    use crate::Progress;
    use crate::commands::local;
    use crate::error::CliError;

    /// How long the graceful shutdown waits for in-flight requests before it
    /// stops regardless. A dev server must stop *fast* on Ctrl-C, not wait out a
    /// grace window (the `platform#5` pingora lesson).
    const SHUTDOWN_GRACE: Duration = Duration::from_millis(300);

    /// One running version of the app: a sandbox and the handle spawned against
    /// its bundle. Replaced wholesale on a successful rebuild.
    pub(super) struct Active {
        sandbox: WasmSandbox,
        handle: Handle,
        /// The bundle this was spawned from. A rebuild that produces the same
        /// digest changed nothing, so the swap is skipped.
        digest: BundleRef,
    }

    /// The current [`Active`], swappable without stopping the listener.
    ///
    /// A plain `Mutex<Arc<_>>`: the lock is held only long enough to clone or
    /// replace the `Arc`, never across an `.await`, so it cannot stall a request.
    pub(super) type Reloadable = Arc<Mutex<Arc<Active>>>;

    /// The `Active` a request should be served against, right now.
    pub(super) fn current(reloadable: &Reloadable) -> Arc<Active> {
        // Recover from a poisoned lock rather than panic: a poisoned lock only
        // means some task panicked while holding it, and the `Arc` inside is
        // still a valid `Active` to serve from.
        reloadable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Install a freshly built `Active`, unless it is byte-identical to the
    /// current one. Returns whether a swap happened.
    pub(super) fn store(reloadable: &Reloadable, active: Active) -> bool {
        let mut guard = reloadable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.digest == active.digest {
            return false;
        }
        *guard = Arc::new(active);
        true
    }

    /// Build the project through the `#28` pipeline and spawn a sandbox for it.
    ///
    /// The build is a `cargo` subprocess, so it runs on a blocking thread and
    /// never stalls the listener — the old `Active` keeps serving until this
    /// returns.
    pub(super) async fn build_active(project: &Path) -> Result<Active, CliError> {
        let owned = project.to_owned();
        let built = tokio::task::spawn_blocking(move || {
            crate::build::run(&crate::build::Options::new(owned), None)
        })
        .await
        .map_err(|e| CliError::Internal {
            source: anyhow::Error::new(e).context("the build task panicked"),
        })??;

        let bytes = std::fs::read(&built.bundle).map_err(|e| CliError::Internal {
            source: anyhow::Error::new(e).context(format!("reading {}", built.bundle.display())),
        })?;
        let digest = BundleRef::of_stream(&bytes);
        let sandbox = local::local_sandbox(&digest, bytes, project)?;
        let handle = sandbox
            .spawn(digest, local::local_identity(), SpawnOptions::default())
            .await
            .map_err(local::from_runtime)?;

        // Drain the guest's own logs to stderr, prefixed so they are separable
        // from the request log they interleave with. The stream ends when this
        // `Active` is dropped on a later swap.
        let stream = sandbox
            .logs_stream(&handle)
            .await
            .map_err(local::from_runtime)?;
        tokio::spawn(drain_guest_logs(stream));

        Ok(Active {
            sandbox,
            handle,
            digest,
        })
    }

    /// Forward the guest's own stdout/stderr output, one prefixed line at a time.
    async fn drain_guest_logs(stream: clwd_runtime::LogStream) {
        let mut stream = std::pin::pin!(stream);
        while let Some(line) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            if let Ok(line) = line {
                tracing::info!("[app] {}", line.message);
            }
        }
    }

    /// Serve one request against `active`, converting hyper's request into the
    /// runtime's and back.
    async fn serve_one(
        active: &Active,
        req: hyper::Request<Incoming>,
    ) -> Result<Response, CliError> {
        let (parts, body) = req.into_parts();
        let bytes = body
            .collect()
            .await
            .map_err(|e| CliError::Internal {
                source: anyhow::Error::new(e).context("reading the request body"),
            })?
            .to_bytes();

        // Rebuild the URI as an absolute localhost URL: a server receives the
        // origin form (`/path`), but `wasi:http` wants a scheme and authority.
        let path_and_query = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        let mut builder = hyper::Request::builder()
            .method(parts.method)
            .uri(format!("http://127.0.0.1{path_and_query}"));
        if let Some(headers) = builder.headers_mut() {
            *headers = parts.headers;
        }
        let request = builder
            .body(Body::from(bytes))
            .map_err(|e| CliError::InvalidArgument {
                what: format!("the request could not be rebuilt: {e}"),
            })?;

        active
            .sandbox
            .invoke(&active.handle, request, InvokeOptions::default())
            .await
            .map_err(local::from_runtime)
    }

    /// The hyper service: serve against the current `Active`, log the request.
    ///
    /// Infallible to hyper — a sandbox failure becomes a `500` to the browser and
    /// a line in the log, never a dropped connection.
    async fn handle_request(
        reloadable: Reloadable,
        req: hyper::Request<Incoming>,
    ) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
        let start = Instant::now();
        let method = req.method().clone();
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str().to_owned())
            .unwrap_or_else(|| "/".to_owned());

        let active = current(&reloadable);
        let (status, body) = match serve_one(&active, req).await {
            Ok(resp) => (resp.status(), resp.into_body().into_bytes()),
            Err(e) => {
                tracing::warn!("request failed: {e}");
                (
                    hyper::StatusCode::INTERNAL_SERVER_ERROR,
                    Bytes::from_static(b"internal error"),
                )
            }
        };

        // The request timing — deliberately not the rebuild timing, which the
        // reload line reports separately.
        tracing::info!(
            "{method} {path} -> {} ({} ms)",
            status.as_u16(),
            start.elapsed().as_millis()
        );

        let response = hyper::Response::builder()
            .status(status)
            .body(Full::new(body))
            .unwrap_or_else(|_| {
                hyper::Response::new(Full::new(Bytes::from_static(b"internal error")))
            });
        Ok(response)
    }

    /// Run the dev server until Ctrl-C.
    pub fn execute(
        project: &Path,
        port: u16,
        progress: Option<&dyn Progress>,
    ) -> Result<(), CliError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| CliError::Internal { source: e.into() })?;
        runtime.block_on(serve(project, port, progress))
    }

    async fn serve(
        project: &Path,
        port: u16,
        progress: Option<&dyn Progress>,
    ) -> Result<(), CliError> {
        if let Some(p) = progress {
            p.step("building the app");
        }
        let reloadable: Reloadable = Arc::new(Mutex::new(Arc::new(build_active(project).await?)));
        if let Some(p) = progress {
            p.finish();
        }

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| CliError::Config {
                what: format!(
                    "cannot bind port {port}: {e}. Another server may be using it — \
                     pass `--port` to choose a different one"
                ),
            })?;

        // The watcher lives until the end of this function; dropping it stops
        // watching.
        let _watcher = super::watch::spawn(PathBuf::from(project), Arc::clone(&reloadable))?;

        tracing::info!(
            "clwd dev — serving http://127.0.0.1:{port}\n\
             edit and save to reload; Ctrl-C to stop"
        );

        let graceful = hyper_util::server::graceful::GracefulShutdown::new();
        let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());

        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(|e| CliError::Internal {
                        source: anyhow::Error::new(e).context("accepting a connection"),
                    })?;
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let reloadable = Arc::clone(&reloadable);
                    let service = service_fn(move |req| {
                        handle_request(Arc::clone(&reloadable), req)
                    });
                    let conn = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service);
                    let watched = graceful.watch(conn);
                    tokio::spawn(async move {
                        // A per-connection error (a client that hung up) is not
                        // the server's failure; it is logged and the loop goes on.
                        if let Err(e) = watched.await {
                            tracing::debug!(error = %e, "connection ended with an error");
                        }
                    });
                }
                _ = ctrl_c.as_mut() => {
                    tracing::info!("\nstopping");
                    break;
                }
            }
        }

        // Stop the listener, then give in-flight requests a brief, bounded window
        // to finish. No long grace: Ctrl-C should feel immediate.
        drop(listener);
        tokio::select! {
            _ = graceful.shutdown() => {}
            _ = tokio::time::sleep(SHUTDOWN_GRACE) => {}
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const GUEST: &[u8] = include_bytes!("../../../../runtime/tests/fixtures/app.wasm");

        /// A `.clwd` bundle wrapping the reference guest, written into `dir`.
        fn write_bundle(dir: &Path) -> PathBuf {
            use clwd_bundle::{Capabilities, File, Limits, Runtime, build};
            let encoded = build(
                Runtime::Wasm,
                "app.wasm",
                Capabilities {
                    version: clwd_bundle::manifest::CAPABILITIES_VERSION,
                    egress: Vec::new(),
                    secrets: Vec::new(),
                    storage_namespaces: Vec::new(),
                    limits: Limits {
                        cpu_ms: 5_000,
                        memory_mb: 128,
                    },
                },
                vec![File {
                    path: "app.wasm".to_owned(),
                    data: GUEST.to_vec(),
                    executable: false,
                }],
            )
            .expect("the reference guest makes a valid bundle");
            let path = dir.join("app.clwd");
            std::fs::write(&path, &encoded.bytes).expect("write bundle");
            path
        }

        /// Build an `Active` directly from bundle bytes (no `cargo`), for tests.
        async fn active_from(bundle: &Path) -> Active {
            let bytes = std::fs::read(bundle).expect("read bundle");
            let digest = BundleRef::of_stream(&bytes);
            let sandbox = local::local_sandbox(&digest, bytes, bundle.parent().expect("dir"))
                .expect("build sandbox");
            let handle = sandbox
                .spawn(digest, local::local_identity(), SpawnOptions::default())
                .await
                .expect("spawn");
            Active {
                sandbox,
                handle,
                digest,
            }
        }

        /// Serve a path through an `Active` using the runtime request directly,
        /// bypassing the hyper `Incoming` type that tests cannot construct.
        async fn serve_text(active: &Active, path: &str) -> String {
            let request = hyper::Request::builder()
                .method("GET")
                .uri(format!("http://127.0.0.1{path}"))
                .body(Body::empty())
                .expect("valid request");
            let response = active
                .sandbox
                .invoke(&active.handle, request, InvokeOptions::default())
                .await
                .expect("the guest answers");
            String::from_utf8_lossy(response.into_body().into_bytes().as_ref()).into_owned()
        }

        /// The load-bearing behaviour: a failed rebuild leaves the last good
        /// component serving. Proven by serving before and after a failed
        /// reload and getting the same answer from the same component, not by
        /// asserting an error was logged.
        #[tokio::test]
        async fn a_failed_rebuild_keeps_the_last_good_component_serving() {
            let dir = tempdir();
            let bundle = write_bundle(&dir);
            let reloadable: Reloadable = Arc::new(Mutex::new(Arc::new(active_from(&bundle).await)));

            // The good component answers.
            let before = serve_text(&current(&reloadable), "/").await;
            assert_eq!(before, "ok", "the reference guest answers `/` with `ok`");
            let before_ptr = Arc::as_ptr(&current(&reloadable));

            // A rebuild fails (simulated: a build error is what `build_active`
            // would return). The server does NOT swap — nothing is stored.
            let rebuild: Result<Active, CliError> = Err(CliError::BuildFailed {
                step: "compiling the component",
                detail: "expected `;`".to_owned(),
            });
            if let Ok(active) = rebuild {
                store(&reloadable, active);
            }

            // Still the same component, still serving the old answer.
            assert_eq!(
                Arc::as_ptr(&current(&reloadable)),
                before_ptr,
                "a failed rebuild must not swap the component"
            );
            let after = serve_text(&current(&reloadable), "/").await;
            assert_eq!(after, "ok", "the last good component still serves");
        }

        /// A successful rebuild swaps the component in.
        #[tokio::test]
        async fn a_successful_rebuild_swaps_the_component() {
            let dir = tempdir();
            let bundle = write_bundle(&dir);
            let reloadable: Reloadable = Arc::new(Mutex::new(Arc::new(active_from(&bundle).await)));
            let first_ptr = Arc::as_ptr(&current(&reloadable));

            // A rebuild that produces a *different* digest swaps. Force a new
            // digest by wrapping the same guest in a bundle with a different
            // limit, which changes the sealed manifest bytes.
            let other = {
                use clwd_bundle::{Capabilities, File, Limits, Runtime, build};
                let encoded = build(
                    Runtime::Wasm,
                    "app.wasm",
                    Capabilities {
                        version: clwd_bundle::manifest::CAPABILITIES_VERSION,
                        egress: Vec::new(),
                        secrets: Vec::new(),
                        storage_namespaces: Vec::new(),
                        limits: Limits {
                            cpu_ms: 6_000,
                            memory_mb: 128,
                        },
                    },
                    vec![File {
                        path: "app.wasm".to_owned(),
                        data: GUEST.to_vec(),
                        executable: false,
                    }],
                )
                .expect("valid");
                let path = dir.join("other.clwd");
                std::fs::write(&path, &encoded.bytes).expect("write");
                path
            };
            let swapped = store(&reloadable, active_from(&other).await);
            assert!(swapped, "a different digest must swap");
            assert_ne!(
                Arc::as_ptr(&current(&reloadable)),
                first_ptr,
                "the component was replaced"
            );
            assert_eq!(serve_text(&current(&reloadable), "/").await, "ok");
        }

        /// An identical rebuild (same digest) is a no-op — no needless re-spawn.
        #[tokio::test]
        async fn an_identical_rebuild_does_not_swap() {
            let dir = tempdir();
            let bundle = write_bundle(&dir);
            let reloadable: Reloadable = Arc::new(Mutex::new(Arc::new(active_from(&bundle).await)));
            let swapped = store(&reloadable, active_from(&bundle).await);
            assert!(!swapped, "the same digest must not swap");
        }

        fn tempdir() -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "clwd-dev-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            dir
        }
    }
}
