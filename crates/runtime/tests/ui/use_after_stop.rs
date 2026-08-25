//! `stop` consumes the handle, so a stopped instance cannot be reached again.
//! A compile failure is the guarantee; a runtime assertion would only prove the
//! guarantee was not needed.

use clwd_runtime::test_util::TestSandbox;
use clwd_runtime::{Body, InvokeOptions, Sandbox, SpawnOptions};

#[tokio::main]
async fn main() {
    let sandbox = TestSandbox::builder().build();
    let handle = sandbox
        .spawn(
            clwd_bundle::BundleRef::of_stream(b""),
            clwd_runtime::IdentityCtx::new(clwd_runtime::Caller::Anonymous),
            SpawnOptions::default(),
        )
        .await
        .unwrap();

    sandbox.stop(handle).await.unwrap();

    let _ = sandbox
        .invoke(
            &handle,
            http::Request::builder().body(Body::empty()).unwrap(),
            InvokeOptions::default(),
        )
        .await;
}
