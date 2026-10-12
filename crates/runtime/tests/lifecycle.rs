//! Lifecycle, error delegation, deadlines, and object safety.

mod common;
use common::*;

use std::pin::pin;
use std::time::Duration;

use clwd_runtime::test_util::TestSandbox;
use clwd_runtime::{
    Body, Deadline, Error, InvokeOptions, LogLevel, LogLine, RequestId, Sandbox, SpawnOptions,
};
use futures_core::Stream;

/// The control plane holds one of these and never branches on the backend.
/// If this stops compiling, that promise is broken.
fn _assert_object_safe(_: &dyn Sandbox) {}

fn request() -> clwd_runtime::Request {
    http::Request::builder()
        .uri("/health")
        .body(Body::empty())
        .expect("valid request")
}

#[tokio::test]
async fn full_lifecycle_spawn_invoke_logs_stop() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder()
        .artifact(&digest, bytes)
        .logs(vec![
            LogLine::new(LogLevel::Info, "listening"),
            LogLine::new(LogLevel::Warn, "slow query"),
        ])
        .responder(|req| {
            http::Response::builder()
                .status(200)
                .body(Body::from(format!("saw {}", req.uri())))
                .expect("valid response")
        })
        .build();

    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let response = sandbox
        .invoke(&handle, request(), InvokeOptions::default())
        .await
        .expect("invokes");
    assert_eq!(response.status(), 200);
    assert_eq!(response.body().as_bytes(), b"saw /health");

    let mut stream = pin!(sandbox.logs_stream(&handle).await.expect("streams"));
    let mut lines = Vec::new();
    while let Some(line) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
        lines.push(line.expect("a good line"));
    }
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].message, "listening");
    assert_eq!(lines[1].level, LogLevel::Warn);

    sandbox.stop(handle).await.expect("stops");
    assert!(sandbox.instances()[0].stopped);
}

/// The property is that the code is *unchanged*, so only a string comparison
/// tests it. Asserting merely that an error occurred would pass even if the
/// code had been re-spelled into the runtime namespace.
#[tokio::test]
async fn a_bundle_failure_keeps_its_own_code_verbatim() {
    let (digest, bytes) = tampered_bundle();
    let sandbox = TestSandbox::builder().artifact(&digest, bytes).build();

    let error = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect_err("a tampered member must not load");

    assert_eq!(error.code(), "bundle.member_digest_mismatch");
    assert!(
        matches!(error, Error::Bundle { .. }),
        "delegation goes through the Bundle variant"
    );
}

#[tokio::test]
async fn an_absent_artifact_is_distinct_from_an_invalid_one() {
    let (digest, _) = valid_bundle();
    let sandbox = TestSandbox::builder().build();

    let error = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect_err("nothing is stored");
    assert_eq!(error.code(), "runtime.bundle_not_found");
}

/// #17 must be able to blow the cold-start budget without blaming the
/// invocation budget, so the phase is part of the error rather than folded in.
#[tokio::test]
async fn a_spawn_deadline_is_reported_as_the_spawn_phase() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder()
        .artifact(&digest, bytes)
        .spawn_cost(Duration::from_secs(30))
        .build();

    let mut opts = SpawnOptions::default();
    opts.deadline = Some(Deadline::after(Duration::from_millis(1)));

    let error = sandbox
        .spawn(digest, user("alice"), opts)
        .await
        .expect_err("cold start overruns");

    assert_eq!(error.code(), "runtime.spawn_deadline_exceeded");
    assert!(
        matches!(error, Error::SpawnDeadlineExceeded { .. }),
        "got {error:?}"
    );
}

#[tokio::test]
async fn an_invoke_deadline_is_reported_as_the_invoke_phase() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder()
        .artifact(&digest, bytes)
        .invoke_cost(Duration::from_secs(30))
        .build();

    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns: the spawn budget is untouched");

    let mut opts = InvokeOptions::default();
    opts.deadline = Some(Deadline::after(Duration::from_millis(1)));
    opts.request_id = Some(RequestId::new("req-1"));

    let error = sandbox
        .invoke(&handle, request(), opts)
        .await
        .expect_err("the invocation overruns");

    assert_eq!(error.code(), "runtime.invoke_deadline_exceeded");
    assert!(
        matches!(error, Error::InvokeDeadlineExceeded { .. }),
        "got {error:?}"
    );
}

/// An instance that dies underneath its handle. Distinct from use-after-stop,
/// which does not compile — see `tests/ui/use_after_stop.rs`.
#[tokio::test]
async fn an_instance_that_dies_reports_stopped() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder()
        .artifact(&digest, bytes)
        .die_after_invokes(1)
        .build();

    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    sandbox
        .invoke(&handle, request(), InvokeOptions::default())
        .await
        .expect("the first invocation lands");

    let error = sandbox
        .invoke(&handle, request(), InvokeOptions::default())
        .await
        .expect_err("the instance is gone");
    assert_eq!(error.code(), "runtime.stopped");
}

/// A deadline is absolute, so it does not renew itself between calls.
#[test]
fn a_deadline_does_not_renew_itself() {
    let deadline = Deadline::after(Duration::from_millis(50));
    let first = deadline.remaining();
    std::thread::sleep(Duration::from_millis(5));
    assert!(
        deadline.remaining() < first,
        "a relative budget would have reset here"
    );
}
