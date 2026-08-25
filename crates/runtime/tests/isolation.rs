//! One test per claim the isolation design makes — and one per thing it
//! explicitly does not claim, so the limits are recorded rather than implied.

mod common;
use common::*;

use clwd_runtime::test_util::TestSandbox;
use clwd_runtime::{Sandbox, SpawnOptions};

/// The claim the whole design exists for.
#[tokio::test]
async fn every_instance_observes_exactly_one_caller() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder().artifact(&digest, bytes).build();

    for subject in ["alice", "bob", "carol"] {
        let handle = sandbox
            .spawn(digest, user(subject), SpawnOptions::default())
            .await
            .expect("spawns");
        sandbox.stop(handle).await.expect("stops");
    }

    let instances = sandbox.instances();
    assert_eq!(instances.len(), 3);
    for record in &instances {
        assert_eq!(
            record.callers.len(),
            1,
            "instance {} observed {} callers",
            record.instance,
            record.callers.len()
        );
    }
}

/// Two callers get two instances. Nothing in the API can merge them, because
/// the only way to reach an instance is a handle and the only way to get a
/// handle is a spawn that consumed one identity.
#[tokio::test]
async fn two_callers_never_share_an_instance() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder().artifact(&digest, bytes).build();

    let alice = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let bob = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_ne!(alice.instance(), bob.instance());
    assert_eq!(subject_of(alice.caller()), Some("alice"));
    assert_eq!(subject_of(bob.caller()), Some("bob"));
}

/// A handle's identity is fixed at spawn and readable but not writable. There
/// is no `identity_mut`, and this test exists so that adding one is a visible
/// change rather than a quiet one.
#[tokio::test]
async fn a_handle_identity_is_read_only_and_fixed_at_spawn() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder().artifact(&digest, bytes).build();

    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let before = handle.caller().clone();
    let _ = sandbox
        .invoke(
            &handle,
            http::Request::builder()
                .body(clwd_runtime::Body::empty())
                .expect("valid request"),
            clwd_runtime::InvokeOptions::default(),
        )
        .await
        .expect("invokes");
    assert_eq!(handle.caller(), &before, "invoke cannot alter the binding");
}

/// Stated non-guarantee: nothing here can see inside a backend. A backend that
/// pools guests internally can hand out two handles that alias one instance,
/// and this test records that the type system does not catch it — the
/// obligation is documented on the trait, not enforced by it.
#[tokio::test]
async fn aliasing_inside_a_backend_is_not_prevented_by_the_types() {
    let (digest, bytes) = valid_bundle();
    let sandbox = TestSandbox::builder().artifact(&digest, bytes).build();

    let alice = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let bob = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("spawns");

    // Distinct here only because this backend behaves. A misbehaving one could
    // return the same InstanceId twice and no signature would object.
    assert_ne!(alice.instance(), bob.instance());
    assert_eq!(
        sandbox.instances().len(),
        2,
        "the recorder is what catches aliasing; the compiler is not"
    );
}
