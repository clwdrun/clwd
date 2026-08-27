//! Secrets and app data, against a real component that calls both interfaces.

#![cfg(feature = "wasm")]

mod wasm_common;
use wasm_common::*;

use clwd_runtime::{InvokeOptions, Sandbox, SpawnOptions};

async fn call(sandbox: &impl Sandbox, handle: &clwd_runtime::Handle, path: &str) -> String {
    let response = sandbox
        .invoke(handle, get(path), InvokeOptions::default())
        .await
        .expect("invokes");
    text(&response)
}

#[tokio::test]
async fn a_guest_reads_a_secret_it_was_given() {
    let (sandbox, digest, _) = sandbox_with_secrets(&["api-key"], &[("api-key", "s3kr1t")]);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_eq!(call(&sandbox, &handle, "/secret/api-key").await, "s3kr1t");
}

/// **The central isolation test.** A secret the app was not given is not
/// merely refused — it is absent, and absence is the same answer as "no such
/// secret". A guest cannot learn which of the two it hit, so it cannot
/// enumerate by probing.
#[tokio::test]
async fn an_ungranted_secret_is_indistinguishable_from_one_that_does_not_exist() {
    // The store holds `other`, but the manifest never declared it, so it is
    // not in the injection set.
    let (sandbox, digest, _) = sandbox_with_secrets(
        &["api-key"],
        &[("api-key", "s3kr1t"), ("other", "not-yours")],
    );
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    let ungranted = call(&sandbox, &handle, "/secret/other").await;
    let nonexistent = call(&sandbox, &handle, "/secret/no-such-secret-anywhere").await;

    assert_eq!(ungranted, "absent");
    assert_eq!(
        ungranted, nonexistent,
        "a guest must not be able to tell an ungranted secret from a missing one"
    );
}

/// The manifest can narrow the injection set but never widen it: declaring a
/// name the store will not authorise yields nothing.
#[tokio::test]
async fn a_declared_secret_the_store_will_not_authorise_is_still_absent() {
    let (sandbox, digest, _) =
        sandbox_with_secrets(&["api-key", "unauthorised"], &[("api-key", "s3kr1t")]);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_eq!(call(&sandbox, &handle, "/secret/api-key").await, "s3kr1t");
    assert_eq!(
        call(&sandbox, &handle, "/secret/unauthorised").await,
        "absent"
    );
}

#[tokio::test]
async fn app_data_round_trips_and_delete_makes_it_absent() {
    let (sandbox, digest, _) = sandbox_with_secrets(&[], &[]);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_eq!(call(&sandbox, &handle, "/data/get/note").await, "absent");
    assert_eq!(
        call(&sandbox, &handle, "/data/set/note/hello").await,
        "stored"
    );
    assert_eq!(call(&sandbox, &handle, "/data/get/note").await, "hello");
    assert_eq!(
        call(&sandbox, &handle, "/data/delete/note").await,
        "deleted"
    );
    assert_eq!(call(&sandbox, &handle, "/data/get/note").await, "absent");

    // Idempotent: deleting what is not there succeeds.
    assert_eq!(
        call(&sandbox, &handle, "/data/delete/note").await,
        "deleted"
    );
}

/// An empty value is legal and is *present* — distinct from absence.
#[tokio::test]
async fn an_empty_value_is_stored_and_is_not_absence() {
    let (sandbox, digest, _) = sandbox_with_secrets(&[], &[]);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_eq!(call(&sandbox, &handle, "/data/set/empty/").await, "stored");
    // The guest renders a present empty value as the empty string, and absence
    // as "absent" — so this asserts the two are different answers.
    assert_eq!(call(&sandbox, &handle, "/data/get/empty").await, "");
}

/// **Documented behaviour, asserted so a later "fix" announces itself.** Two
/// callers of one app share a namespace. If this ever fails, someone has made
/// app data per-caller and changed a published contract.
#[tokio::test]
async fn two_callers_of_one_app_share_a_namespace() {
    let (sandbox, digest, _) = sandbox_with_secrets(&[], &[]);
    let alice = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let bob = sandbox
        .spawn(digest, user("bob"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_eq!(
        call(&sandbox, &alice, "/data/set/note/from-alice").await,
        "stored"
    );
    assert_eq!(
        call(&sandbox, &bob, "/data/get/note").await,
        "from-alice",
        "app data belongs to the app, not the caller — see DECISION 1 in \
         wit/deps/app-data/app-data.wit. If this now fails, a published \
         contract changed."
    );
}

/// Two apps cannot reach each other's keys.
#[tokio::test]
async fn two_apps_have_separate_namespaces() {
    let (sandbox, one, two) = sandbox_with_two_apps();
    let first = sandbox
        .spawn(one, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");
    let second = sandbox
        .spawn(two, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    assert_eq!(
        call(&sandbox, &first, "/data/set/note/app-one").await,
        "stored"
    );
    assert_eq!(
        call(&sandbox, &second, "/data/get/note").await,
        "absent",
        "one app must not see another's keys"
    );
}

#[tokio::test]
async fn illegal_keys_are_rejected_and_legal_edge_cases_are_not() {
    let (sandbox, digest, _) = sandbox_with_secrets(&[], &[]);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    // Uppercase and stray punctuation are out, as are leading and trailing
    // separators. The characters here are all legal in a URI — a key with a
    // space cannot be tested this way because the request would not build,
    // which is a limit of carrying keys in the path, not of the rules.
    for key in ["Note", "a~b", "a+b", "-leading", "trailing-"] {
        let answer = call(&sandbox, &handle, &format!("/data/get/{key}")).await;
        assert!(
            answer.starts_with("error:invalid-key"),
            "{key:?} should be rejected, got {answer:?}"
        );
    }

    // The maximum length is legal.
    let longest = "a".repeat(128);
    assert_eq!(
        call(&sandbox, &handle, &format!("/data/set/{longest}/v")).await,
        "stored"
    );
    // One over is not.
    let too_long = "a".repeat(129);
    assert!(call(&sandbox, &handle, &format!("/data/get/{too_long}"))
        .await
        .starts_with("error:invalid-key"));
}

#[tokio::test]
async fn a_value_over_the_ceiling_is_refused_with_the_limit() {
    let (sandbox, digest, _) = sandbox_with_secrets(&[], &[]);
    let handle = sandbox
        .spawn(digest, user("alice"), SpawnOptions::default())
        .await
        .expect("spawns");

    // Exactly at the floor every host must accept.
    assert_eq!(
        call(&sandbox, &handle, "/data/big/1048576").await,
        "stored",
        "every host accepts values of at least 1 MiB"
    );
    // One byte over this host's limit reports that limit.
    assert_eq!(
        call(&sandbox, &handle, "/data/big/1048577").await,
        "error:value-too-large:1048576"
    );
}
