//! Round trip, per-request state, deadlines, traps, logs and the cache.

#![cfg(feature = "wasm")]

mod wasm_common;
use wasm_common::*;

use std::sync::Arc;
use std::time::Duration;

use clwd_runtime::wasm::{MemoryBundleStore, WasmSandbox};
use clwd_runtime::{
    Deadline, Error, InvokeOptions, LogStreamKind, Phase, RequestId, Sandbox, SpawnOptions,
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

/// A host that spawns and stops once per request must end where it started.
/// Each entry holds a copy of its app's secrets, so a `stop` that only marks
/// the entry grows memory, and the secrets in it, with every request served.
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

#[tokio::test]
async fn a_spawn_deadline_reports_the_spawn_phase() {
    let (sandbox, digest) = sandbox();
    let mut opts = SpawnOptions::default();
    opts.deadline = Some(Deadline::after(Duration::ZERO));

    let error = sandbox
        .spawn(digest, user("alice"), opts)
        .await
        .expect_err("cold start is out of budget");

    assert_eq!(error.code(), "runtime.deadline_exceeded");
    assert!(
        matches!(
            error,
            Error::DeadlineExceeded {
                phase: Phase::Spawn,
                ..
            }
        ),
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

    assert_eq!(error.code(), "runtime.deadline_exceeded");
    assert!(
        matches!(
            error,
            Error::DeadlineExceeded {
                phase: Phase::Invoke,
                ..
            }
        ),
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
