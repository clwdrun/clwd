//! The epoch ticker.
//!
//! wasmtime's epoch interruption works by comparing a counter against a
//! per-`Store` deadline at function entries and loop back-edges. Something has
//! to increment that counter, and *what* increments it is a correctness
//! decision rather than a style one.
//!
//! # Why this is an OS thread and not a tokio task
//!
//! **Do not move this onto the async runtime.** It looks like an obvious
//! simplification and it breaks the mechanism entirely.
//!
//! Guest execution runs on a fiber polled from an executor thread, and between
//! epoch checks it occupies that thread synchronously. A guest in a tight loop
//! with no yield point therefore blocks a tokio worker until an epoch deadline
//! fires. If the ticker is itself a task on that runtime, then once spinning
//! guests outnumber the workers there is nowhere left to poll it: the epoch
//! never increments, no deadline ever fires, no guest is ever preempted, and
//! the workers never come back. **The enforcement mechanism is starved by
//! exactly the condition it exists to handle.**
//!
//! This was measured, not assumed. Running the same spinning guest both ways:
//!
//! | Ticker | 2 guests / 2 workers | 8 guests / 2 workers |
//! | --- | --- | --- |
//! | tokio task | preempted | **no preemption in 10s** |
//! | OS thread | preempted | preempted |
//!
//! Note the first column. At 2 guests on 2 workers the tokio ticker *passes*,
//! because `block_on` drives the runtime on the calling thread and gives it a
//! third place to run. Anyone re-testing this after "simplifying" it needs more
//! spinning guests than the runtime has workers, or they will get a false pass.
//! `tests/wasm_starvation.rs` holds the version that fails.

use std::thread;
use std::time::Duration;

use wasmtime::Engine;

/// One tick is one millisecond.
///
/// A quota cannot be enforced more precisely than one tick. The manifest's
/// `capabilities.limits.cpu_ms` is denominated in milliseconds and floors at one
/// of them, so making a tick a millisecond means the budget needs no unit
/// conversion — a `cpu_ms` value *is* a tick count — and the finest quota the
/// manifest can express is the finest quota this can enforce.
pub(super) const TICK: Duration = Duration::from_millis(1);

/// Start ticking `engine`'s epoch until the engine is dropped.
///
/// The thread holds only a weak handle, so it never keeps the engine alive: when
/// the last real `Engine` goes away `upgrade` returns `None` and the thread
/// exits within one tick. An `Arc<Engine>` here would leak both the thread and
/// the engine for the life of the process.
pub(super) fn spawn(engine: &Engine) {
    let weak = engine.weak();
    thread::Builder::new()
        .name("clwd-epoch".to_owned())
        .spawn(move || {
            loop {
                thread::sleep(TICK);
                // Dropping the upgraded handle each iteration matters: holding
                // it across the sleep would keep the engine alive for a tick
                // beyond its last real owner, and holding it forever would
                // defeat the weak reference entirely.
                match weak.upgrade() {
                    Some(engine) => engine.increment_epoch(),
                    None => break,
                }
            }
        })
        .expect("spawning the epoch ticker");
}
