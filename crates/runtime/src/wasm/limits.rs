//! Per-`Store` resource caps, and how a denial is made identifiable.
//!
//! # Denying with an error rather than a false
//!
//! `ResourceLimiter::memory_growing` has three outcomes. `Ok(false)` makes
//! `memory.grow` return -1, which is what the WebAssembly specification
//! prescribes. `Err(e)` raises a trap carrying `e`. **This limiter returns
//! `Err`, which is a deliberate deviation from the specification.**
//!
//! The reasoning is not that the guest's allocator failed. It is that the guest
//! asked for something its manifest committed to not giving it — a value sealed
//! inside the hashed bundle, which a reviewer looked at. Letting it continue
//! means running untrusted code past a boundary nobody has reasoned about,
//! in whatever degraded state its allocator chose. Stopping is the right
//! answer for code we did not write and cannot audit.
//!
//! The error also buys attribution that no other option offers.
//! `StoreLimitsBuilder::trap_on_grow_failure(true)` traps but says nothing about
//! which limit or how much was asked for. Recording a flag and consulting it
//! when a trap arrives **over-claims**: a guest that hit `Ok(false)`, handled
//! the -1, and later trapped for something unrelated would be reported as out
//! of memory. A typed error is present only on the denial path and cannot drift.
//!
//! # What this does not bound
//!
//! - **It is per-`Store`, not per-host.** A thousand concurrent guests each
//!   inside `memory_mb` can still exhaust the machine. Per-host admission
//!   control belongs to the embedding host, and nothing here substitutes for
//!   it.
//! - **It does not bound host memory the runtime allocates for a guest** — the
//!   resource table, buffered request and response bodies. Those are host
//!   allocations attributed to no `Store` limit. Log capture is bounded
//!   separately; bodies are not.
//! - **It does not bound CPU.** That is the epoch mechanism, in `epoch.rs`.

use wasmtime::ResourceLimiter;

use super::budget::Budget;
use crate::error::Error;

/// Table elements one instance may allocate.
///
/// A second allocation channel that the memory cap does not see: `table.grow`
/// allocates host-side storage for references without touching linear memory.
///
/// Measured rather than guessed: the reference component built from
/// `crates/sdk/wit/world.wit` peaks at **76** table elements serving a request. This is
/// three orders of magnitude above that, which is headroom for a component
/// doing real work rather than a cap anyone is expected to approach.
pub(super) const MAX_TABLE_ELEMENTS: usize = 100_000;

/// Instances one `Store` may hold. A component instantiates its inner
/// instances at startup, so this bounds a component that nests without limit.
///
/// Unlike the table cap this one is **not** measured against a real component —
/// the reference fixture never approaches it, so there is no high-water mark to
/// report. Treat the number as a sanity bound, not as a tuned one.
const MAX_INSTANCES: usize = 64;

/// Table and memory *counts*.
///
/// **Belt-and-braces, not a security control.** A component's table and memory
/// counts are fixed by its type at compile time, so a well-formed component
/// cannot vary them at run time — these bound something that is already bounded.
/// They are set so the limiter is complete rather than because anything is known
/// to reach them; do not treat either as load-bearing.
const MAX_TABLES: usize = 32;
const MAX_MEMORIES: usize = 32;

/// The typed denial. Raised as a trap, downcast on the error path.
///
/// One variant per limit, because the limits differ in who can raise them:
/// memory is the manifest's `memory_mb`, the table cap is fixed by the host.
/// Each maps to its own code.
#[derive(Debug, Clone, Copy)]
pub(super) enum LimitExceeded {
    Memory { requested: u64, allowed: u64 },
    TableElements { requested: u64, allowed: u64 },
}

impl LimitExceeded {
    pub(super) fn into_error(self) -> Error {
        match self {
            Self::Memory { requested, allowed } => {
                Error::MemoryLimitExceeded { requested, allowed }
            }
            Self::TableElements { requested, allowed } => {
                Error::TableLimitExceeded { requested, allowed }
            }
        }
    }
}

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.into_error().fmt(f)
    }
}

impl std::error::Error for LimitExceeded {}

/// Enforces one instance's caps. Lives in the `Store`'s data, so it is
/// per-`Store` and nothing is shared between guests.
#[derive(Debug)]
pub(super) struct BudgetLimiter {
    memory_bytes: usize,
    /// High-water marks, so the fixed caps above can be set from what real
    /// components need rather than from intuition. The reference component
    /// peaks at 1,114,112 bytes of linear memory and 76 table elements.
    pub(super) peak_memory: usize,
    pub(super) peak_table_elements: usize,
}

impl BudgetLimiter {
    pub(super) fn new(budget: &Budget) -> Self {
        Self {
            memory_bytes: budget.memory_bytes,
            peak_memory: 0,
            peak_table_elements: 0,
        }
    }
}

impl ResourceLimiter for BudgetLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.memory_bytes {
            return Err(LimitExceeded::Memory {
                requested: desired as u64,
                allowed: self.memory_bytes as u64,
            }
            .into());
        }
        self.peak_memory = self.peak_memory.max(desired);
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > MAX_TABLE_ELEMENTS {
            return Err(LimitExceeded::TableElements {
                requested: desired as u64,
                allowed: MAX_TABLE_ELEMENTS as u64,
            }
            .into());
        }
        self.peak_table_elements = self.peak_table_elements.max(desired);
        Ok(true)
    }

    fn instances(&self) -> usize {
        MAX_INSTANCES
    }

    fn tables(&self) -> usize {
        MAX_TABLES
    }

    fn memories(&self) -> usize {
        MAX_MEMORIES
    }
}
