//! Resource enforcement: CPU budgets, memory caps, and the failures they
//! produce. The claim is that one guest exhausting its budget is that guest's
//! problem and nobody else's.

#![cfg(feature = "wasm")]

mod wasm_common;
use wasm_common::*;

use std::sync::Arc;
use std::time::{Duration, Instant};

use clwd_runtime::{Error, InvokeOptions, Phase, Sandbox, SpawnOptions};

/// The budget is small so the test is quick; it is still hundreds of times
/// longer than a normal request, which takes ~136µs.
const CPU_MS: u64 = 50;

#[tokio::test]
async fn a_spinning_guest_is_preempted_within_its_budget() {
    let (sandbox, digest) = sandbox_with_limits(CPU_MS, 128);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let started = Instant::now();
    let error = sandbox
        .invoke(&handle, get("/spin"), InvokeOptions::default())
        .await
        .expect_err("an infinite loop must not return");
    let elapsed = started.elapsed();

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
    // Generous: the point is that it terminated at all, not that scheduling is
    // precise. A failure here means preemption did not happen, not that it was
    // slow.
    assert!(
        elapsed < Duration::from_secs(5),
        "preemption took {elapsed:?}"
    );
}

/// A guest that exceeds its sealed memory budget is stopped, and says which
/// limit it hit.
#[tokio::test]
async fn a_guest_allocating_past_its_memory_budget_is_stopped() {
    // The reference component needs ~1.06 MiB to serve a request, so 4 MiB
    // starts cleanly and leaves room for `/alloc` to run into the cap.
    let (sandbox, digest) = sandbox_with_limits(5000, 4);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let error = sandbox
        .invoke(&handle, get("/alloc"), InvokeOptions::default())
        .await
        .expect_err("allocating without bound must not succeed");

    assert_eq!(error.code(), "runtime.resource_exhausted");
    match error {
        Error::ResourceExhausted {
            limit,
            requested,
            allowed,
        } => {
            assert_eq!(limit, "memory");
            assert!(requested > allowed, "{requested} should exceed {allowed}");
            assert_eq!(allowed, 4 * 1024 * 1024);
        }
        other => panic!("got {other:?}"),
    }

    // And the host is still standing: the same sandbox serves another request.
    let response = sandbox
        .invoke(&handle, get("/whoami"), InvokeOptions::default())
        .await
        .expect("the sandbox survives a guest running out of memory");
    assert_eq!(text(&response), "alice");
}

/// The mapping must not swallow real traps. A guest that panics is broken, not
/// over budget, and the two call for different responses.
#[tokio::test]
async fn an_ordinary_trap_is_still_reported_as_a_trap() {
    let (sandbox, digest) = sandbox_with_limits(CPU_MS, 128);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let error = sandbox
        .invoke(&handle, get("/trap"), InvokeOptions::default())
        .await
        .expect_err("the guest panics");

    assert_eq!(
        error.code(),
        "runtime.trap",
        "a panic must not be reported as a budget failure"
    );
}

/// **The isolation claim.** One caller burning its whole budget must not affect
/// another caller on the same bundle.
#[tokio::test]
async fn one_guest_exhausting_its_quota_does_not_affect_a_concurrent_caller() {
    let (sandbox, digest) = sandbox_with_limits(CPU_MS, 128);
    let sandbox = Arc::new(sandbox);

    let spinner = sandbox
        .spawn(digest, user("spinner"), SpawnOptions::default())
        .await
        .expect("spawns");
    let neighbour = sandbox
        .spawn(digest, user("neighbour"), SpawnOptions::default())
        .await
        .expect("spawns");

    let spinning = {
        let sandbox = Arc::clone(&sandbox);
        tokio::spawn(async move {
            sandbox
                .invoke(&spinner, get("/spin"), InvokeOptions::default())
                .await
        })
    };

    // While the neighbour spins, this caller must still be served correctly.
    let response = sandbox
        .invoke(&neighbour, get("/whoami"), InvokeOptions::default())
        .await
        .expect("a neighbour's runaway guest must not block this call");
    assert_eq!(text(&response), "neighbour");

    let error = spinning
        .await
        .expect("task completes")
        .expect_err("the spinner is preempted");
    assert_eq!(error.code(), "runtime.deadline_exceeded");
}

/// Wall clock and execution time are different quantities with one source each,
/// so they compose: whichever expires first ends the call. Here the manifest's
/// CPU budget is generous and the caller's wall-clock deadline is not.
#[tokio::test]
async fn a_caller_deadline_and_a_manifest_budget_compose() {
    let (sandbox, digest) = sandbox_with_limits(60_000, 128);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let mut opts = InvokeOptions::default();
    opts.deadline = Some(clwd_runtime::Deadline::after(Duration::from_millis(100)));

    let error = sandbox
        .invoke(&handle, get("/spin"), opts)
        .await
        .expect_err("the caller's wall clock runs out first");
    assert_eq!(error.code(), "runtime.deadline_exceeded");
}

/// And the other way: no caller deadline at all, so only the sealed CPU budget
/// stops it. A manifest cannot be opted out of by omitting an option.
#[tokio::test]
async fn the_manifest_budget_applies_with_no_caller_deadline() {
    let (sandbox, digest) = sandbox_with_limits(CPU_MS, 128);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let error = sandbox
        .invoke(&handle, get("/spin"), InvokeOptions::default())
        .await
        .expect_err("the sealed budget stops it");
    assert_eq!(error.code(), "runtime.deadline_exceeded");
}
