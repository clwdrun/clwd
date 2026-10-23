//! Round trip, per-request state, deadlines, traps, logs and the cache.

#![cfg(feature = "wasm")]

mod wasm_common;
use wasm_common::*;

use std::sync::Arc;
use std::time::Duration;

use clwd_runtime::wasm::{
    AppIdRequired, MemoryAppDataStore, MemoryBundleStore, MemorySecretStore, WasmSandbox,
};
use clwd_runtime::{
    AppId, Deadline, Error, InvokeOptions, LogStreamKind, MAX_RESPONSE_BODY, RequestId, Sandbox,
    SpawnOptions,
};
use futures_core::Stream;

#[tokio::test]
async fn a_full_round_trip_serves_a_response() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let response = sandbox
        .invoke(&handle, get("/"), InvokeOptions::default())
        .await
        .expect("invokes");

    assert_eq!(response.status(), 200);
    assert_eq!(text(&response), "ok");
}

/// Agents pattern-match old templates, so an app built against the previous
/// identity interface will be deployed. It must fail at spawn with an error
/// that names the interface and its version, not trap mid-request.
///
/// Invariant: RT-17.
#[tokio::test]
async fn a_component_built_against_identity_0_1_is_refused_at_spawn() {
    let (digest, bytes) = bundle_of(GUEST_IDENTITY_0_1);
    let bundles = MemoryBundleStore::new();
    bundles.insert(&digest, bytes);
    let sandbox = WasmSandbox::new(Arc::new(bundles)).expect("engine builds");

    let err = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect_err("a 0.1.0 component must not spawn");
    assert_eq!(err.code(), "runtime.incompatible_interface", "{err}");
    let message = err.to_string();
    assert!(
        message.contains("clwd:identity/") && message.contains("@0.1.0"),
        "the error must name the interface and its version: {message}"
    );
    assert!(message.contains("clwd-sdk"), "{message}");
    assert_eq!(sandbox.live_instances(), 0);
}

/// A host with persistent stores installs `AppIdRequired`, so a spawn that
/// forgets to name its app fails instead of falling back to a digest.
///
/// Invariant: RT-12.
#[tokio::test]
async fn app_id_required_refuses_a_spawn_that_names_no_app() {
    let (digest, bytes) = bundle_of(GUEST);
    let bundles = MemoryBundleStore::new();
    bundles.insert(&digest, bytes);
    let sandbox = WasmSandbox::with_stores(
        Arc::new(bundles),
        Arc::new(MemorySecretStore::new()),
        Arc::new(MemoryAppDataStore::new()),
        Arc::new(AppIdRequired),
    )
    .expect("engine builds");

    let err = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect_err("a spawn with no app must fail");
    assert!(err.to_string().contains("named no app"), "{err}");
    assert_eq!(sandbox.live_instances(), 0);

    let mut opts = SpawnOptions::default();
    opts.app = Some(AppId::new("app-a"));
    sandbox
        .spawn(digest, user("alice"), opts)
        .await
        .expect("a spawn that names its app runs");
}

/// Responses are buffered, so the cap is what keeps a guest from choosing how
/// much host memory a request takes. At the cap is served; one byte over fails.
///
/// Invariant: RT-8.
#[tokio::test]
async fn a_response_body_past_the_cap_is_refused() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    // Past wasi-http's 1 MiB body buffer, so this also covers reading the body
    // while the guest writes it. Read after `handle` returns instead, and the
    // guest blocks on a full channel: the deadline turns that hang into a
    // failure.
    let mut opts = InvokeOptions::default();
    opts.deadline = Some(Deadline::after(Duration::from_secs(20)));
    let at_cap = sandbox
        .invoke(
            &handle,
            get(&format!("/big-response/{MAX_RESPONSE_BODY}")),
            opts,
        )
        .await
        .expect("a body at the cap is served");
    assert_eq!(at_cap.body().len(), MAX_RESPONSE_BODY);

    // Not `expect_err`: on failure it would print the 16 MiB body.
    let Err(err) = sandbox
        .invoke(
            &handle,
            get(&format!("/big-response/{}", MAX_RESPONSE_BODY + 1)),
            InvokeOptions::default(),
        )
        .await
    else {
        panic!("a body past the cap was served");
    };
    assert!(
        matches!(
            err,
            Error::ResponseTooLarge { allowed } if allowed == MAX_RESPONSE_BODY as u64
        ),
        "{err:?}"
    );
    assert_eq!(err.code(), "runtime.response_too_large");
    assert!(!err.retryable(), "the same body hits the same fixed cap");
    assert!(
        err.to_string().contains(&MAX_RESPONSE_BODY.to_string()),
        "the message carries the cap, so the hint does not have to: {err}"
    );
}

/// A host that spawns and stops once per request must end where it started.
/// Each entry holds a copy of its app's secrets, so a `stop` that only marks
/// the entry grows memory, and the secrets in it, with every request served.
///
/// Invariant: RT-18.
#[tokio::test]
async fn stop_removes_the_instance() {
    let (sandbox, digest) = sandbox();
    for subject in ["alice", "bob", "carol"] {
        let handle = sandbox
            .spawn(digest, user(subject), SpawnOptions::default())
            .await
            .expect("spawns");
        sandbox
            .invoke(&handle, get("/"), InvokeOptions::default())
            .await
            .expect("invokes");
        assert_eq!(sandbox.live_instances(), 1);
        sandbox.stop(handle).await.expect("stops");
    }
    assert_eq!(sandbox.live_instances(), 0, "stop left instances behind");
}

/// **This test pins the promise made by per-invocation instantiation.**
///
/// The module documents that nothing a guest writes survives into its next
/// request, because every invocation gets fresh linear memory. The guest's
/// `/counter` increments a `static mut` and returns it, so a long-lived `Store`
/// would return 1 then 2.
///
/// If this ever fails with `2`, someone has switched to a reused instance for
/// performance and silently withdrawn a documented guarantee. That is the
/// failure this assertion exists to make loud — it is not an observation about
/// counters.
///
/// Invariant: RT-1.
#[tokio::test]
async fn guest_state_never_survives_into_the_next_request() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let first = sandbox
        .invoke(&handle, get("/counter"), InvokeOptions::default())
        .await
        .expect("invokes");
    let second = sandbox
        .invoke(&handle, get("/counter"), InvokeOptions::default())
        .await
        .expect("invokes");

    assert_eq!(text(&first), "1");
    assert_eq!(
        text(&second),
        "1",
        "guest state survived into the next request: the per-invocation \
         instantiation guarantee documented on the wasm module has been withdrawn"
    );
}

/// A guest has no ambient authority: no filesystem, no environment, no
/// arguments, no sockets and no name lookup. Everything it reaches is an
/// interface the host grants per manifest. The guest imports every one of
/// these interfaces and the linker provides them, so this is the context the
/// host builds that denies them, not a missing import.
///
/// Invariant: RT-15.
#[tokio::test]
async fn a_guest_has_no_ambient_authority() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let response = sandbox
        .invoke(&handle, get("/ambient"), InvokeOptions::default())
        .await
        .expect("invokes");
    assert_eq!(
        text(&response),
        "fs-root:false fs-cwd:false env:0 args:0 tcp:false udp:false dns:false"
    );
}

#[tokio::test]
async fn a_spawn_deadline_reports_the_spawn_phase() {
    let (sandbox, digest) = sandbox();
    let mut opts = SpawnOptions::default();
    opts.deadline = Some(Deadline::after(Duration::ZERO));

    let error = sandbox
        .spawn(digest, user("alice"), opts)
        .await
        .expect_err("cold start is out of budget");

    assert_eq!(error.code(), "runtime.spawn_deadline_exceeded");
    assert!(error.retryable(), "spawn_deadline_exceeded retryable");
    assert!(
        matches!(error, Error::SpawnDeadlineExceeded { .. }),
        "got {error:?}"
    );
}

#[tokio::test]
async fn an_invoke_deadline_reports_the_invoke_phase() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("the spawn budget is untouched");

    let mut opts = InvokeOptions::default();
    opts.deadline = Some(Deadline::after(Duration::ZERO));

    let error = sandbox
        .invoke(&handle, get("/"), opts)
        .await
        .expect_err("the invocation is out of budget");

    assert_eq!(error.code(), "runtime.invoke_deadline_exceeded");
    assert!(error.retryable(), "invoke_deadline_exceeded retryable");
    assert!(
        matches!(error, Error::InvokeDeadlineExceeded { .. }),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_trapping_guest_yields_runtime_trap_with_its_message() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let error = sandbox
        .invoke(&handle, get("/trap"), InvokeOptions::default())
        .await
        .expect_err("the guest panics");

    assert_eq!(error.code(), "runtime.trap");
    let message = error.to_string();
    assert!(
        message.contains("unreachable") || message.contains("trap"),
        "the trap message must survive: got {message:?}"
    );
}

/// A trap is a guest failure, not an engine failure. If it poisoned the cached
/// component or the `Engine`, one caller's bug would take out every other
/// caller of the same bundle — the cross-caller failure this whole backend is
/// built to prevent.
///
/// Invariant: RT-14.
#[tokio::test]
async fn a_trap_does_not_poison_the_cache_for_other_callers() {
    let (sandbox, digest) = sandbox();

    let alice = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    sandbox
        .invoke(&alice, get("/trap"), InvokeOptions::default())
        .await
        .expect_err("alice's guest traps");

    // A different caller, spawning after the trap, on the same bundle.
    let bob = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("bob can still spawn after another caller's trap");
    let response = sandbox
        .invoke(&bob, get("/whoami"), InvokeOptions::default())
        .await
        .expect("bob can still invoke after another caller's trap");
    assert_eq!(text(&response), "bob");

    // And alice's own handle still works: the trap killed one instance, which
    // existed only for that call.
    let again = sandbox
        .invoke(&alice, get("/whoami"), InvokeOptions::default())
        .await
        .expect("alice's handle survives her own trap");
    assert_eq!(text(&again), "alice");
}

#[tokio::test]
async fn logs_carry_the_request_id_and_the_stream_they_came_from() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let mut stream = std::pin::pin!(sandbox.logs_stream(&handle).await.expect("subscribes"));

    let mut opts = InvokeOptions::default();
    opts.request_id = Some(RequestId::new("req-42"));
    sandbox
        .invoke(&handle, get("/log"), opts)
        .await
        .expect("invokes");

    let mut lines = Vec::new();
    while lines.len() < 2 {
        match tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)),
        )
        .await
        {
            Ok(Some(line)) => lines.push(line.expect("a good line")),
            _ => break,
        }
    }

    assert_eq!(lines.len(), 2, "one line from each stream: {lines:?}");
    for line in &lines {
        assert_eq!(
            line.request_id.as_ref().map(RequestId::as_str),
            Some("req-42"),
            "every line from an invocation carries its request id"
        );
    }
    assert!(
        lines
            .iter()
            .any(|l| l.stream == Some(LogStreamKind::Stdout) && l.message.contains("stdout"))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.stream == Some(LogStreamKind::Stderr) && l.message.contains("stderr"))
    );
}

#[tokio::test]
async fn two_spawns_of_one_bundle_compile_once() {
    let (sandbox, digest) = sandbox();

    let _a = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let _b = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_eq!(sandbox.cached_components().await, 1);
    assert_eq!(
        sandbox.compilations(),
        1,
        "the second spawn must reuse the cached component"
    );
}

/// Single-flight: concurrent spawns of an uncached bundle share one
/// compilation rather than each doing their own and discarding the losers.
#[tokio::test]
async fn concurrent_spawns_of_an_uncached_bundle_compile_once() {
    let (digest, bytes) = bundle_of(GUEST);
    let store = MemoryBundleStore::new();
    store.insert(&digest, bytes);
    let sandbox = Arc::new(WasmSandbox::new(Arc::new(store)).expect("engine builds"));

    let mut tasks = Vec::new();
    for i in 0..8 {
        let sandbox = Arc::clone(&sandbox);
        tasks.push(tokio::spawn(async move {
            sandbox
                .spawn(digest, user(&format!("s{i}")), SpawnOptions::default())
                .await
                .expect("spawns")
        }));
    }
    for task in tasks {
        task.await.expect("task completes");
    }

    assert_eq!(sandbox.cached_components().await, 1);
    assert_eq!(
        sandbox.compilations(),
        1,
        "eight concurrent spawns compiled more than once"
    );
}

/// Defence in depth, gate one: `clwd-bundle` refuses a core module at ingest,
/// so it never reaches wasmtime through `spawn`.
///
/// Invariant: RT-17.
#[tokio::test]
async fn a_core_module_is_rejected_at_ingest() {
    let (digest, bytes) = bundle_of(CORE_MODULE);
    let store = MemoryBundleStore::new();
    store.insert(&digest, bytes);
    let sandbox = WasmSandbox::new(Arc::new(store)).expect("engine builds");

    let error = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect_err("a core module is not a component");
    assert_eq!(error.code(), "bundle.wasm_not_component");
}
