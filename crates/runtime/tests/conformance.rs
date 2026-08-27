//! The backend conformance suite: one test per [`REQUIREMENTS`] entry, run
//! against every backend arm.
//!
//! Today the only arm is `wasm`. When #21 adds the microVM arm this file's
//! `cfg` widens to `feature = "conformance"` alone and `arms()` grows a second
//! entry; every test already loops arms and skips the ones that are not built,
//! so nothing else changes.
//!
//! What each test asserts is the *observable* guarantee — response bodies,
//! error codes, recorded observations — never a wasm internal. A test that
//! passes here against a second backend is evidence that backend conforms; a
//! test that only ever passes because it never really exercised the guest is a
//! bug in the test, which is what the Phase 5 deliberately-broken arm checks.

#![cfg(all(feature = "conformance", feature = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use futures_core::Stream;

use clwd_runtime::conformance::{
    requirement, BackendUnderTest, FixtureKind, WorldSpec, REQUIREMENTS,
};
use clwd_runtime::identity::{Caller, GroupId, IdentityCtx, OrgId, Principal, Role, SubjectId};
use clwd_runtime::{
    Body, Deadline, Error, InvokeOptions, LogLine, LogStream, Request, RequestId, Sandbox,
    SpawnOptions,
};

mod arm_broken; // SCRATCH — Phase 5 experiment
mod arm_wasm;

/// Every backend arm the suite knows about. Unbuilt arms still appear here and
/// report every requirement as skipped.
fn arms() -> Vec<Box<dyn BackendUnderTest>> {
    // SCRATCH — Phase 5: select a deliberately-broken arm to prove the suite
    // can fail. Reverted after the run.
    match std::env::var("CONFORMANCE_ARM").as_deref() {
        Ok("identity-by-bundle") => vec![Box::new(arm_broken::IdentityByBundle::new())],
        Ok("bundle-code-remap") => vec![Box::new(arm_broken::BundleCodeRemap::new())],
        Ok("trap-for-deadline") => vec![Box::new(arm_broken::TrapForInvokeDeadline::new())],
        _ => vec![Box::new(arm_wasm::WasmArm::new())],
    }
}

/// Skip line for an arm that is not built in this binary. Returns `true` when
/// the caller should `continue` past this arm.
fn skipped(arm: &dyn BackendUnderTest, id: &str) -> bool {
    if arm.available() {
        return false;
    }
    eprintln!(
        "SKIP  {id}  [{}]  (arm not built)  — {}",
        arm.name(),
        requirement(id).summary
    );
    true
}

/// Prefix for an assertion message, so a failure names the requirement and the
/// backend rather than reading as a bare mismatch.
fn at(arm: &dyn BackendUnderTest, id: &str) -> String {
    let r = requirement(id);
    format!(
        "{} does not conform to {} ({}): ",
        arm.name(),
        r.id,
        r.summary
    )
}

// --- identity constructors ------------------------------------------------

fn user(subject: &str) -> IdentityCtx {
    IdentityCtx::new(Caller::User(Principal::new(
        SubjectId::new(subject),
        OrgId::new("acme"),
    )))
}

fn user_with(subject: &str, role: Role, groups: &[&str]) -> IdentityCtx {
    let mut ctx = user(subject);
    ctx.role = Some(role);
    ctx.groups = groups.iter().map(|g| GroupId::new(*g)).collect();
    ctx
}

// --- request / response helpers -----------------------------------------

fn get(path: &str) -> Request {
    http::Request::builder()
        .method("GET")
        .uri(format!("http://app.test{path}"))
        .body(Body::empty())
        .expect("valid request")
}

fn with_deadline(d: Duration) -> InvokeOptions {
    let mut o = InvokeOptions::default();
    o.deadline = Some(Deadline::after(d));
    o
}

fn with_request_id(id: &str) -> InvokeOptions {
    let mut o = InvokeOptions::default();
    o.request_id = Some(RequestId::new(id));
    o
}

fn body_of(r: &clwd_runtime::Response) -> String {
    String::from_utf8(r.body().as_bytes().to_vec()).expect("utf-8 body")
}

/// Drive one invocation and return its body, panicking with `ctx` on error.
async fn call(sb: &Arc<dyn Sandbox>, h: &clwd_runtime::Handle, path: &str, ctx: &str) -> String {
    let r = sb
        .invoke(h, get(path), InvokeOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{ctx}{path} errored: {e:?}"));
    body_of(&r)
}

/// Collect up to `max` log lines, or until `timeout` elapses with nothing new.
async fn drain_logs(stream: LogStream, max: usize, timeout: Duration) -> Vec<LogLine> {
    let mut stream = std::pin::pin!(stream);
    let mut out = Vec::new();
    while out.len() < max {
        match tokio::time::timeout(
            timeout,
            std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)),
        )
        .await
        {
            Ok(Some(line)) => out.push(line.expect("a good log line")),
            _ => break,
        }
    }
    out
}

// ======================================================================
// Registry <-> tests: the mapping must be total and one-to-one.
// ======================================================================

/// Every requirement id that has a `#[tokio::test]` below. A new requirement in
/// `REQUIREMENTS` must be added here with its test; a test whose id is not in
/// `REQUIREMENTS` is a stale reference. `every_requirement_has_exactly_one_test`
/// holds the two together.
const COVERED: &[&str] = &[
    "R1.1", "R1.2", "R1.3", "R1.4", "R1.5", "R1.6", "R1.7", "R1.8", //
    "R2.1", "R2.2", "R2.3", "R2.4", "R2.5", "R2.6", //
    "R3.1", "R3.2", "R3.3", //
    "R4.1", "R4.2", "R4.3", "R4.4", "R4.5", "R4.6", //
    "R5.1", "R5.2", "R5.3", //
    "R6.1", "R6.2", "R6.3", "R6.4", "R6.5", "R6.6", //
    "R7.1", "R7.2",
];

#[test]
fn every_requirement_has_exactly_one_test() {
    use std::collections::BTreeSet;
    let covered: BTreeSet<&str> = COVERED.iter().copied().collect();
    let declared: BTreeSet<&str> = REQUIREMENTS.iter().map(|r| r.id).collect();

    assert_eq!(covered.len(), COVERED.len(), "COVERED has a duplicate id");
    let missing: Vec<_> = declared.difference(&covered).collect();
    let stale: Vec<_> = covered.difference(&declared).collect();
    assert!(missing.is_empty(), "requirements with no test: {missing:?}");
    assert!(
        stale.is_empty(),
        "tests naming an unknown requirement: {stale:?}"
    );
}

// ======================================================================
// R1 — caller isolation
// ======================================================================

#[tokio::test]
async fn r1_1() {
    const ID: &str = "R1.1";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        for who in ["alice", "bob"] {
            let h = sb
                .spawn(guest(&*arm), user(who), SpawnOptions::default())
                .await
                .expect("spawns");
            // Exercise the instance a few times: a pooling backend that aliased
            // it to another caller would record a second caller here.
            for _ in 0..3 {
                call(&sb, &h, "/whoami", &at(&*arm, ID)).await;
            }
        }
        for obs in arm.instances() {
            assert_eq!(
                obs.callers.len(),
                1,
                "{}instance {} observed {:?}",
                at(&*arm, ID),
                obs.instance,
                obs.callers
            );
        }
    }
}

#[tokio::test]
async fn r1_2() {
    const ID: &str = "R1.2";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let a = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns a");
        let b = sb
            .spawn(guest(&*arm), user("bob"), SpawnOptions::default())
            .await
            .expect("spawns b");
        assert_ne!(
            a.instance(),
            b.instance(),
            "{}two callers share an instance",
            at(&*arm, ID)
        );
        let ids: Vec<_> = arm.instances().into_iter().map(|o| o.instance).collect();
        assert!(
            ids.contains(a.instance()) && ids.contains(b.instance()),
            "{}both instances must be recorded, got {ids:?}",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r1_3() {
    const ID: &str = "R1.3";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let before = h.instance().clone();
        // An invoke is the only other operation on a handle; it must not be
        // able to change which instance or identity the handle names.
        call(&sb, &h, "/whoami", &at(&*arm, ID)).await;
        assert_eq!(h.instance(), &before, "{}instance id moved", at(&*arm, ID));
        assert_eq!(
            h.caller(),
            &Caller::User(Principal::new(SubjectId::new("alice"), OrgId::new("acme"))),
            "{}caller moved",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r1_4() {
    const ID: &str = "R1.4";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("carol"), SpawnOptions::default())
            .await
            .expect("spawns");
        assert_eq!(
            call(&sb, &h, "/whoami", &at(&*arm, ID)).await,
            "carol",
            "{}guest saw a different subject than its handle carries",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r1_5() {
    const ID: &str = "R1.5";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let a = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns a");
        let b = sb
            .spawn(guest(&*arm), user("bob"), SpawnOptions::default())
            .await
            .expect("spawns b");
        // Alternate handles: a backend that resolved identity from anything
        // other than the handle passed in would cross here.
        for _ in 0..8 {
            assert_eq!(call(&sb, &a, "/whoami", &at(&*arm, ID)).await, "alice");
            assert_eq!(call(&sb, &b, "/whoami", &at(&*arm, ID)).await, "bob");
            assert_eq!(call(&sb, &b, "/whoami", &at(&*arm, ID)).await, "bob");
            assert_eq!(call(&sb, &a, "/whoami", &at(&*arm, ID)).await, "alice");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r1_6() {
    const ID: &str = "R1.6";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let a = Arc::new(
            sb.spawn(guest(&*arm), user("alice"), SpawnOptions::default())
                .await
                .expect("spawns a"),
        );
        let b = Arc::new(
            sb.spawn(guest(&*arm), user("bob"), SpawnOptions::default())
                .await
                .expect("spawns b"),
        );
        // Many overlapping pairs: a data race in identity plumbing shows up as
        // one response carrying the other caller's subject.
        let mut tasks = Vec::new();
        for _ in 0..40 {
            for (h, want) in [(a.clone(), "alice"), (b.clone(), "bob")] {
                let sb = sb.clone();
                tasks.push(tokio::spawn(async move {
                    let got = body_of(
                        &sb.invoke(&h, get("/whoami"), InvokeOptions::default())
                            .await
                            .expect("invokes"),
                    );
                    assert_eq!(got, want, "concurrent invocation crossed identities");
                }));
            }
        }
        for t in tasks {
            t.await.expect("no identity crossing under concurrency");
        }
    }
}

#[tokio::test]
async fn r1_7() {
    const ID: &str = "R1.7";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(
                guest(&*arm),
                user_with("dave", Role::Editor, &["eng", "oncall"]),
                SpawnOptions::default(),
            )
            .await
            .expect("spawns");
        assert_eq!(
            call(&sb, &h, "/role", &at(&*arm, ID)).await,
            "editor",
            "{}role did not come from the spawning identity",
            at(&*arm, ID)
        );
        assert_eq!(
            call(&sb, &h, "/groups", &at(&*arm, ID)).await,
            "eng,oncall",
            "{}groups did not come from the spawning identity",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r1_8() {
    const ID: &str = "R1.8";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(
                guest(&*arm),
                IdentityCtx::new(Caller::Anonymous),
                SpawnOptions::default(),
            )
            .await
            .expect("spawns");
        assert_eq!(
            call(&sb, &h, "/whoami", &at(&*arm, ID)).await,
            "anonymous",
            "{}an anonymous caller must arrive as anonymous",
            at(&*arm, ID)
        );
        assert_eq!(
            call(&sb, &h, "/role", &at(&*arm, ID)).await,
            "none",
            "{}anonymous has no role",
            at(&*arm, ID)
        );
    }
}

// ======================================================================
// R2 — lifecycle & errors
// ======================================================================

#[tokio::test]
async fn r2_1() {
    const ID: &str = "R2.1";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let stream = sb.logs_stream(&h).await.expect("logs stream opens");
        assert_eq!(call(&sb, &h, "/", &at(&*arm, ID)).await, "ok");
        sb.invoke(&h, get("/log"), with_request_id("rt"))
            .await
            .expect("invokes");
        let lines = drain_logs(stream, 4, Duration::from_secs(2)).await;
        assert!(
            !lines.is_empty(),
            "{}a round trip produced no log lines",
            at(&*arm, ID)
        );
        sb.stop(h).await.expect("stops");
    }
}

#[tokio::test]
async fn r2_2() {
    const ID: &str = "R2.2";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        // `/exit` is the guest asking to go away; the arm makes that real.
        let _ = sb.invoke(&h, get("/exit"), InvokeOptions::default()).await;
        arm.evict(h.instance()).await;
        let err = sb
            .invoke(&h, get("/"), InvokeOptions::default())
            .await
            .expect_err("an invoke after the instance is gone must fail");
        assert_eq!(
            err.code(),
            "runtime.stopped",
            "{}got {err:?}",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r2_3() {
    const ID: &str = "R2.3";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        // A well-formed digest for a bundle the store does not hold.
        let absent = clwd_runtime::BundleRef::of_stream(b"no artifact under this digest");
        let err = sb
            .spawn(absent, user("alice"), SpawnOptions::default())
            .await
            .expect_err("spawning an absent artifact must fail");
        assert_eq!(
            err.code(),
            "runtime.bundle_not_found",
            "{}got {err:?}",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r2_4() {
    const ID: &str = "R2.4";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let mut codes = Vec::new();
        for kind in [FixtureKind::IngestFailure, FixtureKind::Tampered] {
            let (digest, _) = arm.fixture_bundle(kind);
            let err = match sb
                .spawn(digest, user("alice"), SpawnOptions::default())
                .await
            {
                Ok(_) => panic!("{}spawning {kind:?} succeeded", at(&*arm, ID)),
                Err(e) => e,
            };
            assert!(
                err.code().starts_with("bundle."),
                "{}{kind:?} surfaced {:?}, not a bundle.* code",
                at(&*arm, ID),
                err.code()
            );
            codes.push(err.code());
        }
        assert_ne!(
            codes[0],
            codes[1],
            "{}the two ingest failures collapsed to one code: {codes:?}",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r2_5() {
    const ID: &str = "R2.5";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let mut opts = SpawnOptions::default();
        opts.deadline = Some(Deadline::after(Duration::ZERO)); // already spent
        let err = sb
            .spawn(guest(&*arm), user("alice"), opts)
            .await
            .expect_err("a spent spawn deadline must fail the spawn");
        assert_eq!(
            err.code(),
            "runtime.deadline_exceeded",
            "{}got {err:?}",
            at(&*arm, ID)
        );
        assert!(
            matches!(
                err,
                Error::DeadlineExceeded {
                    phase: clwd_runtime::Phase::Spawn,
                    ..
                }
            ),
            "{}deadline fired in the wrong phase: {err:?}",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r2_6() {
    const ID: &str = "R2.6";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let err = sb
            .invoke(&h, get("/"), with_deadline(Duration::ZERO))
            .await
            .expect_err("a spent invoke deadline must fail the invoke");
        assert_eq!(
            err.code(),
            "runtime.deadline_exceeded",
            "{}got {err:?}",
            at(&*arm, ID)
        );
        assert!(
            matches!(
                err,
                Error::DeadlineExceeded {
                    phase: clwd_runtime::Phase::Invoke,
                    ..
                }
            ),
            "{}deadline fired in the wrong phase: {err:?}",
            at(&*arm, ID)
        );
    }
}

// ======================================================================
// R3 — fault isolation
// ======================================================================

#[tokio::test]
async fn r3_1() {
    const ID: &str = "R3.1";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let err = sb
            .invoke(&h, get("/trap"), InvokeOptions::default())
            .await
            .expect_err("a crashing guest must fail the invoke");
        assert_eq!(err.code(), "runtime.trap", "{}got {err:?}", at(&*arm, ID));
        match err {
            Error::Trap { message } => assert!(
                !message.is_empty(),
                "{}a trap must carry a non-empty message",
                at(&*arm, ID)
            ),
            other => panic!("{}got {other:?}", at(&*arm, ID)),
        }
    }
}

#[tokio::test]
async fn r3_2() {
    const ID: &str = "R3.2";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let alice = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns alice");
        let bob = sb
            .spawn(guest(&*arm), user("bob"), SpawnOptions::default())
            .await
            .expect("spawns bob");
        sb.invoke(&alice, get("/trap"), InvokeOptions::default())
            .await
            .expect_err("alice's guest traps");
        assert_eq!(
            call(&sb, &bob, "/whoami", &at(&*arm, ID)).await,
            "bob",
            "{}another caller of the same bundle was taken down by the crash",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r3_3() {
    const ID: &str = "R3.3";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        // Tight memory budget: a crash here must still be a trap, not a budget
        // failure the mapping over-claimed.
        let mut w = WorldSpec::defaults();
        w.memory_mb = 4;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let err = sb
            .invoke(&h, get("/trap"), InvokeOptions::default())
            .await
            .expect_err("the guest panics");
        assert_eq!(
            err.code(),
            "runtime.trap",
            "{}a panic was reported as {:?}",
            at(&*arm, ID),
            err.code()
        );
    }
}

// ======================================================================
// R4 — resource budgets
// ======================================================================

#[tokio::test]
async fn r4_1() {
    const ID: &str = "R4.1";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let mut w = WorldSpec::defaults();
        w.cpu_ms = 50;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let started = std::time::Instant::now();
        let err = sb
            .invoke(&h, get("/spin"), InvokeOptions::default())
            .await
            .expect_err("a never-yielding guest must not return");
        assert_eq!(
            err.code(),
            "runtime.deadline_exceeded",
            "{}got {err:?}",
            at(&*arm, ID)
        );
        assert!(
            matches!(
                err,
                Error::DeadlineExceeded {
                    phase: clwd_runtime::Phase::Invoke,
                    ..
                }
            ),
            "{}wrong phase: {err:?}",
            at(&*arm, ID)
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{}preemption took {:?}",
            at(&*arm, ID),
            started.elapsed()
        );
    }
}

#[tokio::test]
async fn r4_2() {
    const ID: &str = "R4.2";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        // No caller deadline at all — only the sealed CPU budget can stop a
        // spinning guest.
        let mut w = WorldSpec::defaults();
        w.cpu_ms = 50;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let err = sb
            .invoke(&h, get("/spin"), InvokeOptions::default())
            .await
            .expect_err("the sealed budget must stop a spin with no caller deadline");
        assert_eq!(
            err.code(),
            "runtime.deadline_exceeded",
            "{}got {err:?}",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r4_3() {
    const ID: &str = "R4.3";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        // Case A — generous CPU, tiny wall clock, a guest that sleeps: the
        // wall-clock deadline is what ends the call.
        let mut w = WorldSpec::defaults();
        w.cpu_ms = 5_000;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let err = sb
            .invoke(
                &h,
                get("/sleep/10000"),
                with_deadline(Duration::from_millis(200)),
            )
            .await
            .expect_err("a 200ms wall-clock deadline must end a 10s sleep");
        assert_eq!(
            err.code(),
            "runtime.deadline_exceeded",
            "{}wall-clock case: got {err:?}",
            at(&*arm, ID)
        );

        // Case B — generous wall clock, tiny CPU, a guest that spins: the
        // sealed CPU budget is what ends the call.
        let mut w = WorldSpec::defaults();
        w.cpu_ms = 50;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let err = sb
            .invoke(&h, get("/spin"), with_deadline(Duration::from_secs(30)))
            .await
            .expect_err("the 50ms CPU budget must end a spin before a 30s wall clock");
        assert_eq!(
            err.code(),
            "runtime.deadline_exceeded",
            "{}cpu-budget case: got {err:?}",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r4_4() {
    const ID: &str = "R4.4";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let mut w = WorldSpec::defaults();
        w.memory_mb = 4;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let err = sb
            .invoke(&h, get("/alloc"), InvokeOptions::default())
            .await
            .expect_err("an unbounded allocation must not succeed");
        assert_eq!(
            err.code(),
            "runtime.resource_exhausted",
            "{}got {err:?}",
            at(&*arm, ID)
        );
        match err {
            Error::ResourceExhausted {
                limit,
                requested,
                allowed,
            } => {
                assert_eq!(limit, "memory", "{}wrong limit name", at(&*arm, ID));
                assert_eq!(
                    allowed,
                    4 * 1024 * 1024,
                    "{}allowed must be the sealed byte count",
                    at(&*arm, ID)
                );
                assert!(
                    requested > allowed,
                    "{}{requested} !> {allowed}",
                    at(&*arm, ID)
                );
            }
            other => panic!("{}got {other:?}", at(&*arm, ID)),
        }
        // The host still serves other requests.
        assert_eq!(call(&sb, &h, "/whoami", &at(&*arm, ID)).await, "alice");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r4_5() {
    const ID: &str = "R4.5";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let mut w = WorldSpec::defaults();
        w.cpu_ms = 100;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let hog = sb
            .spawn(guest(&*arm), user("hog"), SpawnOptions::default())
            .await
            .expect("spawns hog");
        let good = sb
            .spawn(guest(&*arm), user("good"), SpawnOptions::default())
            .await
            .expect("spawns good");
        let sb2 = sb.clone();
        let spin = tokio::spawn(async move {
            sb2.invoke(&hog, get("/spin"), InvokeOptions::default())
                .await
                .expect_err("hog is preempted")
        });
        // The unrelated caller is served correctly while the hog burns down.
        for _ in 0..5 {
            assert_eq!(
                call(&sb, &good, "/whoami", &at(&*arm, ID)).await,
                "good",
                "{}a concurrent caller was affected by another's CPU exhaustion",
                at(&*arm, ID)
            );
        }
        spin.await.expect("hog task joined");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r4_6() {
    const ID: &str = "R4.6";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let mut w = WorldSpec::defaults();
        w.cpu_ms = 100;
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        // More non-yielding guests than the runtime has worker threads.
        let mut spinners = Vec::new();
        for i in 0..6 {
            let h = sb
                .spawn(
                    guest(&*arm),
                    user(&format!("spin{i}")),
                    SpawnOptions::default(),
                )
                .await
                .expect("spawns spinner");
            let sb2 = sb.clone();
            spinners.push(tokio::spawn(async move {
                sb2.invoke(&h, get("/spin"), InvokeOptions::default())
                    .await
                    .expect_err("every spinner is preempted")
            }));
        }
        let victim = sb
            .spawn(guest(&*arm), user("victim"), SpawnOptions::default())
            .await
            .expect("spawns victim");
        let started = std::time::Instant::now();
        assert_eq!(
            call(&sb, &victim, "/whoami", &at(&*arm, ID)).await,
            "victim",
            "{}the host stopped serving while workers were monopolised",
            at(&*arm, ID)
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{}an unrelated caller waited {:?} behind the spinners",
            at(&*arm, ID),
            started.elapsed()
        );
        for s in spinners {
            s.await.expect("spinner joined");
        }
    }
}

// ======================================================================
// R5 — secrets
// ======================================================================

#[tokio::test]
async fn r5_1() {
    const ID: &str = "R5.1";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let mut w = WorldSpec::defaults();
        w.declared_secrets = vec!["api-key".into()];
        w.store_secrets = vec![("api-key".into(), b"s3cr3t".to_vec())];
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        assert_eq!(
            call(&sb, &h, "/secret/api-key", &at(&*arm, ID)).await,
            "s3cr3t",
            "{}a secret in both manifest and store must be readable",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r5_2() {
    const ID: &str = "R5.2";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let mut w = WorldSpec::defaults();
        w.declared_secrets = vec![]; // manifest declares nothing
        w.store_secrets = vec![("api-key".into(), b"s3cr3t".to_vec())];
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        assert_eq!(
            call(&sb, &h, "/secret/api-key", &at(&*arm, ID)).await,
            "absent",
            "{}an undeclared secret must be absent",
            at(&*arm, ID)
        );
        assert_eq!(
            call(&sb, &h, "/secret/never-existed", &at(&*arm, ID)).await,
            "absent",
            "{}absent-because-undeclared must look identical to never-existed",
            at(&*arm, ID)
        );
    }
}

#[tokio::test]
async fn r5_3() {
    const ID: &str = "R5.3";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let mut w = WorldSpec::defaults();
        w.declared_secrets = vec!["api-key".into()]; // declared
        w.store_secrets = vec![]; // but the store does not hold it
        let sb: Arc<dyn Sandbox> = arm.sandbox(&w).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        assert_eq!(
            call(&sb, &h, "/secret/api-key", &at(&*arm, ID)).await,
            "absent",
            "{}a declared-but-unheld secret must be absent",
            at(&*arm, ID)
        );
        assert_eq!(
            call(&sb, &h, "/secret/never-existed", &at(&*arm, ID)).await,
            "absent",
            "{}absent-because-unheld must look identical to never-existed",
            at(&*arm, ID)
        );
    }
}

// ======================================================================
// R6 — app data
// ======================================================================

#[tokio::test]
async fn r6_1() {
    const ID: &str = "R6.1";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let c = at(&*arm, ID);
        assert_eq!(call(&sb, &h, "/data/set/k/v", &c).await, "stored");
        assert_eq!(call(&sb, &h, "/data/get/k", &c).await, "v");
        assert_eq!(call(&sb, &h, "/data/delete/k", &c).await, "deleted");
        assert_eq!(call(&sb, &h, "/data/get/k", &c).await, "absent");
        assert_eq!(
            call(&sb, &h, "/data/delete/k", &c).await,
            "deleted",
            "{c}deleting an unset key must still succeed"
        );
    }
}

#[tokio::test]
async fn r6_2() {
    const ID: &str = "R6.2";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let c = at(&*arm, ID);
        assert_eq!(
            call(&sb, &h, "/data/set/empty/", &c).await,
            "stored",
            "{c}an empty value must be storable"
        );
        assert_eq!(
            call(&sb, &h, "/data/get/empty", &c).await,
            "",
            "{c}an empty value must read back as empty, not absent"
        );
        assert_eq!(
            call(&sb, &h, "/data/get/unset", &c).await,
            "absent",
            "{c}an unset key is absent, distinct from an empty value"
        );
    }
}

#[tokio::test]
async fn r6_3() {
    const ID: &str = "R6.3";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        // One app (one bundle), two callers: they share a namespace.
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let alice = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns alice");
        let bob = sb
            .spawn(guest(&*arm), user("bob"), SpawnOptions::default())
            .await
            .expect("spawns bob");
        let c = at(&*arm, ID);
        assert_eq!(
            call(&sb, &alice, "/data/set/shared/from-alice", &c).await,
            "stored"
        );
        assert_eq!(
            call(&sb, &bob, "/data/get/shared", &c).await,
            "from-alice",
            "{c}two callers of one app must see one namespace"
        );
    }
}

#[tokio::test]
async fn r6_4() {
    const ID: &str = "R6.4";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        // Two apps: distinct bundles (different declared secrets => different
        // digests => different app ids), same sandbox.
        let mut wa = WorldSpec::defaults();
        wa.declared_secrets = vec!["a".into()];
        let mut wb = WorldSpec::defaults();
        wb.declared_secrets = vec!["b".into()];
        let sb_a: Arc<dyn Sandbox> = arm.sandbox(&wa).await.into();
        let a_digest = arm.fixture_bundle(FixtureKind::Guest).0;
        let sb_b: Arc<dyn Sandbox> = arm.sandbox(&wb).await.into();
        let b_digest = arm.fixture_bundle(FixtureKind::Guest).0;
        assert_ne!(a_digest, b_digest, "the two apps must be distinct bundles");

        let ha = sb_a
            .spawn(a_digest, user("alice"), SpawnOptions::default())
            .await
            .expect("spawns in app a");
        let hb = sb_b
            .spawn(b_digest, user("alice"), SpawnOptions::default())
            .await
            .expect("spawns in app b");
        let c = at(&*arm, ID);
        assert_eq!(call(&sb_a, &ha, "/data/set/k/in-a", &c).await, "stored");
        assert_eq!(
            call(&sb_b, &hb, "/data/get/k", &c).await,
            "absent",
            "{c}a key set in one app must not be visible in another"
        );
    }
}

#[tokio::test]
async fn r6_5() {
    const ID: &str = "R6.5";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let c = at(&*arm, ID);
        // Keys the documented grammar forbids must be refused everywhere. All
        // of these are legal URI path segments — only the key grammar rejects
        // them (uppercase, leading/trailing separator, out-of-set characters).
        for bad in ["UPPER", ".lead", "trail.", "a~b", "a+b"] {
            let got = call(&sb, &h, &format!("/data/get/{bad}"), &c).await;
            assert!(
                got.starts_with("error:invalid-key"),
                "{c}key {bad:?} was not refused: {got:?}"
            );
        }
        // The 128-byte boundary: 128 is legal, 129 is not.
        let k128 = "a".repeat(128);
        let k129 = "a".repeat(129);
        assert_eq!(
            call(&sb, &h, &format!("/data/set/{k128}/v"), &c).await,
            "stored",
            "{c}a 128-byte key must be accepted"
        );
        assert!(
            call(&sb, &h, &format!("/data/set/{k129}/v"), &c)
                .await
                .starts_with("error:invalid-key"),
            "{c}a 129-byte key must be refused"
        );
    }
}

#[tokio::test]
async fn r6_6() {
    const ID: &str = "R6.6";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let c = at(&*arm, ID);
        assert_eq!(
            call(&sb, &h, &format!("/data/big/{}", 1024 * 1024), &c).await,
            "stored",
            "{c}a 1 MiB value must be accepted"
        );
        let over = call(&sb, &h, &format!("/data/big/{}", 64 * 1024 * 1024), &c).await;
        let limit = over
            .strip_prefix("error:value-too-large:")
            .unwrap_or_else(|| panic!("{c}an over-limit value was not refused: {over:?}"));
        let limit: u64 = limit
            .parse()
            .unwrap_or_else(|_| panic!("{c}limit not a number: {over:?}"));
        assert!(
            limit >= 1024 * 1024,
            "{c}the reported limit {limit} is below the 1 MiB floor"
        );
    }
}

// ======================================================================
// R7 — logs
// ======================================================================

#[tokio::test]
async fn r7_1() {
    const ID: &str = "R7.1";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let stream = sb.logs_stream(&h).await.expect("logs stream opens");
        sb.invoke(&h, get("/log"), with_request_id("req-42"))
            .await
            .expect("invokes");
        let lines = drain_logs(stream, 8, Duration::from_secs(2)).await;
        assert!(
            !lines.is_empty(),
            "{}the invocation produced no lines",
            at(&*arm, ID)
        );
        for line in &lines {
            assert_eq!(
                line.request_id.as_ref().map(RequestId::as_str),
                Some("req-42"),
                "{}a line from the invocation is missing its request id: {line:?}",
                at(&*arm, ID)
            );
        }
    }
}

#[tokio::test]
async fn r7_2() {
    const ID: &str = "R7.2";
    for arm in arms() {
        if skipped(&*arm, ID) {
            continue;
        }
        let sb: Arc<dyn Sandbox> = arm.sandbox(&WorldSpec::defaults()).await.into();
        let h = sb
            .spawn(guest(&*arm), user("alice"), SpawnOptions::default())
            .await
            .expect("spawns");
        let stream = sb.logs_stream(&h).await.expect("logs stream opens");
        sb.invoke(&h, get("/flood-logs"), InvokeOptions::default())
            .await
            .expect("invokes");
        let lines = drain_logs(stream, 512, Duration::from_secs(3)).await;
        assert!(
            lines.iter().any(|l| l.message.contains("dropped")),
            "{}a flood that overran the buffer produced no drop marker (saw {} lines)",
            at(&*arm, ID),
            lines.len()
        );
    }
}

// --- arm-agnostic: the current guest digest for the last-built sandbox ---

/// The guest bundle digest the arm's most recent `sandbox()` seeded. Every test
/// spawns with this.
fn guest(arm: &dyn BackendUnderTest) -> clwd_runtime::BundleRef {
    arm.fixture_bundle(FixtureKind::Guest).0
}
