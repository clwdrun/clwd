//! The central claim: a guest observes exactly the identity its `Store` was
//! built with, and never another caller's — proven against a real component
//! that reads `clwd:identity/context` and reports what it saw.

#![cfg(feature = "wasm")]

mod wasm_common;
use wasm_common::*;

use clwd_runtime::identity::Role;
use clwd_runtime::{InvokeOptions, Sandbox, SpawnOptions};

#[tokio::test]
async fn each_guest_reads_its_own_identity() {
    let (sandbox, digest) = sandbox();

    let alice = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let bob = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("spawns");

    let a = sandbox
        .invoke(&alice, get("/whoami"), InvokeOptions::default())
        .await
        .expect("invokes");
    let b = sandbox
        .invoke(&bob, get("/whoami"), InvokeOptions::default())
        .await
        .expect("invokes");

    assert_eq!(text(&a), "alice");
    assert_eq!(text(&b), "bob");
}

/// The harder ordering: interleave the two, and assert the first caller never
/// picks up the second's identity from a reused instance.
#[tokio::test]
async fn interleaved_invocations_never_cross_identities() {
    let (sandbox, digest) = sandbox();

    let alice = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let bob = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("spawns");

    let first = sandbox
        .invoke(&alice, get("/whoami"), InvokeOptions::default())
        .await
        .expect("invokes");
    let middle = sandbox
        .invoke(&bob, get("/whoami"), InvokeOptions::default())
        .await
        .expect("invokes");
    let again = sandbox
        .invoke(&alice, get("/whoami"), InvokeOptions::default())
        .await
        .expect("invokes");

    assert_eq!(text(&first), "alice");
    assert_eq!(text(&middle), "bob");
    assert_eq!(
        text(&again),
        "alice",
        "alice must not observe bob after an interleaved call"
    );
}

/// Concurrency is where a pooling backend would leak. Each invocation builds
/// its own `Store`, so overlapping calls cannot see each other.
#[tokio::test]
async fn concurrent_invocations_never_cross_identities() {
    let (sandbox, digest) = sandbox();
    let sandbox = std::sync::Arc::new(sandbox);

    let mut handles = Vec::new();
    for i in 0..8 {
        let subject = format!("subject-{i}");
        let sandbox = std::sync::Arc::clone(&sandbox);
        handles.push(tokio::spawn(async move {
            let handle = sandbox
                .spawn(digest, user(&subject), SpawnOptions::default())
                .await
                .expect("spawns");
            let response = sandbox
                .invoke(&handle, get("/whoami"), InvokeOptions::default())
                .await
                .expect("invokes");
            (subject, text(&response))
        }));
    }

    for task in handles {
        let (expected, observed) = task.await.expect("task completes");
        assert_eq!(observed, expected, "a guest read another caller's identity");
    }
}

/// Role and groups travel with the identity, and are read from the same
/// per-`Store` state as the caller.
#[tokio::test]
async fn role_and_groups_come_from_the_spawning_identity() {
    let (sandbox, digest) = sandbox();

    let editor = sandbox
        .spawn(
            digest,
            user_with("alice", Role::Editor, &["eng", "oncall"]),
            SpawnOptions::default(),
        )
        .await
        .expect("spawns");
    let plain = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("spawns");

    let role = sandbox
        .invoke(&editor, get("/role"), InvokeOptions::default())
        .await
        .expect("invokes");
    assert_eq!(text(&role), "editor");

    let groups = sandbox
        .invoke(&editor, get("/groups"), InvokeOptions::default())
        .await
        .expect("invokes");
    assert_eq!(text(&groups), "eng,oncall");

    let none = sandbox
        .invoke(&plain, get("/role"), InvokeOptions::default())
        .await
        .expect("invokes");
    assert_eq!(
        text(&none),
        "none",
        "bob has no role and must not see alice's"
    );
}

/// Anonymous is reachable and must arrive as itself, not as an empty user.
#[tokio::test]
async fn an_anonymous_caller_arrives_as_anonymous() {
    let (sandbox, digest) = sandbox();
    let handle = sandbox
        .spawn(
            digest,
            clwd_runtime::IdentityCtx::new(clwd_runtime::Caller::Anonymous),
            SpawnOptions::default(),
        )
        .await
        .expect("spawns");

    let response = sandbox
        .invoke(&handle, get("/whoami"), InvokeOptions::default())
        .await
        .expect("invokes");
    assert_eq!(text(&response), "anonymous");
}
