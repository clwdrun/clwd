//! Epoch overhead against #54's baseline. Ignored by default: it is a
//! measurement, and a timing assertion in CI is a flake waiting to happen.
//!
//! Run with:
//!   cargo test --release -p clwd-runtime --features wasm \
//!     --test wasm_overhead -- --ignored --nocapture
#![cfg(feature = "wasm")]
mod wasm_common;
use clwd_runtime::{InvokeOptions, Sandbox, SpawnOptions};
use std::time::{Duration, Instant};
use wasm_common::*;

async fn sample(path: &str, cpu_ms: u64) -> (Duration, Duration, Duration) {
    let (sandbox, digest) = sandbox_with_limits(cpu_ms, 128);
    let handle = sandbox
        .spawn(digest, user("bench"), SpawnOptions::default())
        .await
        .expect("spawns");
    for _ in 0..5 {
        let _ = sandbox
            .invoke(&handle, get(path), InvokeOptions::default())
            .await;
    }
    let mut s = Vec::new();
    for _ in 0..50 {
        let t = Instant::now();
        let _ = sandbox
            .invoke(&handle, get(path), InvokeOptions::default())
            .await;
        s.push(t.elapsed());
    }
    s.sort();
    (s[0], s[25], s[47])
}

#[tokio::test]
#[ignore]
async fn invoke_overhead() {
    let (min, p50, p95) = sample("/", 5000).await;
    println!("short request  min/p50/p95: {min:?} / {p50:?} / {p95:?}");

    let (cmin, cp50, cp95) = sample("/counter", 5000).await;
    println!("counter        min/p50/p95: {cmin:?} / {cp50:?} / {cp95:?}");
}

/// A guest that crosses many deadlines, so the `Yield` path's cost shows up
/// rather than only the flag's presence. A short request may never reach a
/// single deadline, which would make the overhead look like zero.
#[tokio::test]
#[ignore]
async fn yield_path_cost() {
    for cpu_ms in [20u64, 100, 500] {
        let (sandbox, digest) = sandbox_with_limits(cpu_ms, 128);
        let handle = sandbox
            .spawn(digest, user("bench"), SpawnOptions::default())
            .await
            .expect("spawns");
        let t = Instant::now();
        let err = sandbox
            .invoke(&handle, get("/spin"), InvokeOptions::default())
            .await
            .expect_err("preempted");
        let elapsed = t.elapsed();
        println!(
            "budget {cpu_ms:>4}ms -> wall {elapsed:?} (overshoot {:?}), {}",
            elapsed.saturating_sub(Duration::from_millis(cpu_ms)),
            err.code()
        );
    }
}
