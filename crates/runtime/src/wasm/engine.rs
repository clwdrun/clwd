//! `Engine` construction.
//!
//! Every flag here is compile-affecting: changing one invalidates every
//! compiled `Component` in the cache, because `Component::deserialize` checks a
//! compatibility hash and refuses a mismatch. That is why flags this issue does
//! not use are still set — see below.

use std::num::NonZeroUsize;

use wasmtime::{Config, Engine};

use crate::error::Error;

/// Build the engine every `Store` and every compiled `Component` belongs to.
///
/// # Flags set for #17, deliberately, and why they cannot wait
///
/// `epoch_interruption` is **compile-affecting**. If this issue shipped an
/// `Engine` without it, #17 would have to rebuild the `Engine` to add it, and
/// rebuilding the `Engine` invalidates every cached `Component` — compilation
/// is specific to engine configuration. Setting it now costs nothing and saves
/// #17 a cache-invalidating change. Do not remove it as unused.
///
/// `InstanceAllocationStrategy` is deliberately **not** set. Pooling is a
/// runtime allocator choice rather than a compile-affecting one, so #17 can add
/// it without invalidating anything — the opposite case, and the reason the two
/// are treated differently here.
pub(super) fn build() -> Result<Engine, Error> {
    let mut config = Config::new();

    // Note: `Config::async_support` is deprecated in 48 and documented as
    // having no effect — async is unconditional now, so there is nothing to
    // turn on. Phase 0 listed it as a compile-affecting flag; on 48 it is not a
    // flag at all.

    // For #17. See the note above — this cannot be added later without
    // discarding every compiled component.
    config.epoch_interruption(true);

    // Trap messages carry a backtrace, which `invoke` surfaces as part of
    // `runtime.trap`. Compile-affecting, and the reason a trap is actionable
    // rather than just "something went wrong". 20 is wasmtime's default; it is
    // set explicitly so a future default change does not silently shorten the
    // one diagnostic an app author gets.
    config.wasm_backtrace_max_frames(NonZeroUsize::new(20));

    Engine::new(&config).map_err(|e| Error::Internal {
        source: format!("wasmtime engine: {e}").into(),
    })
}
