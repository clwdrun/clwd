//! What a guest is allowed to consume, and where the numbers come from.
//!
//! # Two quantities, not one
//!
//! **Wall clock and execution time are different things.** A guest blocked on
//! host I/O burns wall clock and no CPU; a guest in a tight loop burns both.
//!
//! - [`crate::Deadline`], carried by `SpawnOptions` and `InvokeOptions`, is
//!   wall clock. It is enforced at the host by a timeout around the call.
//! - `capabilities.limits.cpu_ms`, sealed in the bundle manifest, is execution
//!   time. It is enforced inside the guest by the epoch mechanism.
//!
//! Today there is exactly one source of each, so they **compose** — whichever
//! expires first ends the call — rather than competing.
//!
//! # Most restrictive wins
//!
//! Where two sources ever bound the same quantity, the smaller applies and
//! neither can raise a ceiling the other set. A caller must not be able to grant
//! a guest more than its manifest asked for, because the manifest is what the
//! digest sealed and what a reviewer looked at; and a manifest must not be able
//! to override a tighter budget an operator set for reasons the app cannot see.
//!
//! **That rule is written down but not yet exercised**: no two sources bound the
//! same quantity today. If a caller-supplied CPU budget is ever added to
//! `InvokeOptions`, this is where it meets `cpu_ms`, and the rule above is the
//! answer.

use std::time::Duration;

use clwd_bundle::Limits;

/// The resolved budget for one instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Budget {
    /// Execution-time budget, in epoch ticks. One tick is one millisecond, so
    /// this is `cpu_ms` with no conversion — see [`super::epoch::TICK`].
    pub(super) cpu_ticks: u64,
    /// The same value as a `Duration`, for error reporting.
    pub(super) cpu: Duration,
    /// Maximum linear memory, in bytes.
    pub(super) memory_bytes: usize,
}

impl Budget {
    /// Resolve the budget from the manifest limits the bundle sealed.
    pub(super) fn from_manifest(limits: &Limits) -> Self {
        Self {
            cpu_ticks: limits.cpu_ms,
            cpu: Duration::from_millis(limits.cpu_ms),
            // `memory_mb` is bounded to 65536 by the bundle spec, so this
            // cannot overflow a usize on any target that can host a runtime.
            memory_bytes: (limits.memory_mb as usize).saturating_mul(1024 * 1024),
        }
    }
}
