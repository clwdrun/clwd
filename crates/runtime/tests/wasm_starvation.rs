//! The test that justifies the epoch ticker being an OS thread.
//!
//! Every executor worker is saturated with a spinning guest. If the ticker ran
//! as a task on that same runtime there would be nowhere left to poll it, the
//! epoch would stop advancing, and nothing would ever be preempted — the
//! enforcement mechanism starved by exactly the condition it exists to handle.
//!
//! **This is written to fail rather than hang.** A hang is a finding, not an
//! inconvenience, so the result is collected from outside the runtime with a
//! bounded wait: if preemption does not happen the assertion fails and says so,
//! instead of blocking CI until it is killed.
//!
//! Note the shape of the setup. An earlier version of this used as many
//! spinning guests as workers and **passed with a tokio-task ticker**, because
//! `block_on` drives the runtime on the calling thread and gave the ticker a
//! third place to run. Anyone re-testing this needs comfortably more spinning
//! guests than workers or they will get a false pass.

#![cfg(feature = "wasm")]

mod wasm_common;
use wasm_common::*;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use clwd_runtime::{InvokeOptions, Sandbox, SpawnOptions};

const WORKERS: usize = 2;
const SPINNERS: usize = 8;
const CPU_MS: u64 = 50;

/// Invariant: RT-5.
#[test]
fn spinning_guests_cannot_starve_the_epoch_ticker() {
    let (tx, rx) = mpsc::channel();

    // The runtime lives on its own thread so the collecting thread here is not
    // one of its workers — otherwise this test could not observe a hang.
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKERS)
            .enable_all()
            .build()
            .expect("runtime builds");

        runtime.block_on(async move {
            let (sandbox, digest) = sandbox_with_limits(CPU_MS, 128);
            let sandbox = Arc::new(sandbox);

            let mut tasks = Vec::new();
            for i in 0..SPINNERS {
                let sandbox = Arc::clone(&sandbox);
                tasks.push(tokio::spawn(async move {
                    let handle = sandbox
                        .spawn(
                            digest,
                            user(&format!("spinner-{i}")),
                            SpawnOptions::default(),
                        )
                        .await
                        .expect("spawns");
                    sandbox
                        .invoke(&handle, get("/spin"), InvokeOptions::default())
                        .await
                }));
            }

            let mut codes = Vec::new();
            for task in tasks {
                match task.await.expect("task completes") {
                    Ok(_) => codes.push("unexpected success".to_owned()),
                    Err(e) => codes.push(e.code().to_owned()),
                }
            }
            let _ = tx.send(codes);
        });
    });

    let codes = rx.recv_timeout(Duration::from_secs(60)).expect(
        "no guest was preempted within 60s: the epoch stopped advancing while every worker \
             was in wasm, which is what happens when the ticker shares the executor with the \
             guests it is meant to interrupt",
    );

    assert_eq!(codes.len(), SPINNERS);
    for code in &codes {
        assert_eq!(
            code, "runtime.cpu_budget_exceeded",
            "every spinning guest must be preempted; got {codes:?}"
        );
    }
}
