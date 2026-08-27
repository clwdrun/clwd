//! The backend conformance suite's harness surface.
//!
//! `Sandbox` is a trait plus five PRs of tests written against one backend. This
//! module is the other half: an executable definition of what *conforming*
//! behaviour is, so a second backend has something to be measured against and a
//! behaviour only one backend has is caught as a trait bug rather than shipped
//! as a feature.
//!
//! # The two contracts
//!
//! - **Contract A — [`REQUIREMENTS`].** What every backend must do. The suite
//!   (`tests/conformance.rs`) enumerates this list and has exactly one test per
//!   entry, parameterised over backend.
//! - **Contract B — `tests/conformance/FIXTURE.md`.** What a conforming test
//!   *guest* must do. Every requirement is exercised by driving a guest and
//!   asserting on the response, so the guest must behave identically whichever
//!   backend hosts it.
//!
//! # This is a reusable seam
//!
//! It lives in `src/` behind a non-default `conformance` feature, not in
//! `tests/`, so the platform can depend on `clwd-runtime` with `conformance`
//! on, implement [`BackendUnderTest`] for a backend it did not write, and run
//! the same suite against it. Moving this to its own crate later is mechanical —
//! the seam is designed once, here.
//!
//! # What is *not* here
//!
//! Per-backend guarantees that are stronger than conformance. The wasm backend
//! rebuilds linear memory on every invocation, so nothing a guest writes
//! survives into its next request; a long-lived microVM serving one caller will
//! not offer that and is not non-conforming for it. Such properties stay as
//! backend-specific tests next to the backend, annotated as wasm-only.

use async_trait::async_trait;
use clwd_bundle::BundleRef;

use crate::handle::InstanceId;
use crate::identity::Caller;
use crate::Sandbox;

/// One backend, wired up so the suite can build it and observe it.
///
/// The suite holds a `Vec<Box<dyn BackendUnderTest>>` — today `wasm` and
/// `microvm` — and runs every requirement against each. An arm whose
/// [`available`](Self::available) is `false` is reported as *skipped*, visibly,
/// for every requirement; it is never counted as passing.
#[async_trait]
pub trait BackendUnderTest: Send + Sync {
    /// Short name for failure and skip messages: `"wasm"`, `"microvm"`.
    fn name(&self) -> &'static str;

    /// Is this backend implemented in this build?
    ///
    /// `false` when the backend's Cargo feature is off, or when the
    /// implementation is still a stub. The suite checks this before calling
    /// [`sandbox`](Self::sandbox) and emits a skip line per requirement when it
    /// is `false`, so an unproven backend is loud in normal `cargo test`
    /// output rather than invisible.
    fn available(&self) -> bool;

    /// Build a sandbox for one test, seeded from `world`.
    ///
    /// Instances spawned on the returned sandbox must be recorded so that
    /// [`instances`](Self::instances) can report them — an arm is expected to
    /// wrap its real backend in a recording decorator (the way the scripted
    /// `test_util::TestSandbox` records `InstanceRecord`). Call at most once per
    /// arm value; the suite builds `backends()` fresh for each test.
    ///
    /// May only be called when [`available`](Self::available) is `true`.
    async fn sandbox(&self, world: &WorldSpec) -> Box<dyn Sandbox>;

    /// A bundle this backend can spawn, in the shape `kind` asks for.
    ///
    /// `FixtureKind::Guest` is the reference guest of `FIXTURE.md`. The failure
    /// kinds carry a backend-appropriate broken bundle for `R2.4`: which
    /// `bundle.*` code each one yields is this backend's to pick, since the set
    /// of ingest failures a backend can reach depends on what bundles it
    /// accepts.
    fn fixture_bundle(&self, kind: FixtureKind) -> (BundleRef, Vec<u8>);

    /// Every caller each instance of the last-built sandbox has observed.
    ///
    /// `R1.1` requires every entry's `callers` to have exactly one element. The
    /// field is a `Vec` so a violation is representable and therefore
    /// assertable.
    fn instances(&self) -> Vec<InstanceObservation>;
}

/// What the suite wants the world to contain when it asks for a sandbox.
///
/// The harness translates this into whatever its backend needs — a
/// `clwd_bundle::Capabilities` for the sealed manifest, entries in a secret
/// store, and so on. `#[non_exhaustive]`: build with [`WorldSpec::defaults`] and
/// assign the fields a test cares about.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WorldSpec {
    /// `capabilities.limits.cpu_ms` sealed into the guest's manifest.
    pub cpu_ms: u64,
    /// `capabilities.limits.memory_mb` sealed into the guest's manifest.
    pub memory_mb: u64,
    /// Secret names the guest's manifest declares.
    pub declared_secrets: Vec<String>,
    /// Secrets the backing store holds, by name. A secret is readable by the
    /// guest only when it appears in both this list and `declared_secrets`; a
    /// secret in just one is absent to the guest (`R5.2`, `R5.3`). The
    /// two-sided intersection is the mechanism — conformance requires only the
    /// observable outcome.
    pub store_secrets: Vec<(String, Vec<u8>)>,
}

impl WorldSpec {
    /// A world with generous limits and no secrets — the starting point most
    /// tests tweak one field of.
    pub fn defaults() -> Self {
        Self {
            cpu_ms: 5_000,
            memory_mb: 128,
            declared_secrets: Vec::new(),
            store_secrets: Vec::new(),
        }
    }
}

impl Default for WorldSpec {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Which fixture bundle the suite is asking a backend for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FixtureKind {
    /// The reference guest specified by `tests/conformance/FIXTURE.md`.
    Guest,
    /// Bytes that fail `clwd-bundle` ingest validation outright (e.g. not a
    /// tar stream, or a missing manifest). For `R2.4`.
    IngestFailure,
    /// A well-formed bundle with one member byte flipped, so the member no
    /// longer matches its declared digest. For `R2.4`
    /// (`bundle.member_digest_mismatch`).
    Tampered,
}

/// Every caller one instance was handed to, as recorded by a backend arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstanceObservation {
    pub instance: InstanceId,
    /// The caller-isolation invariant says this is always exactly one.
    pub callers: Vec<Caller>,
}

/// One line of contract A: a guarantee every backend owes.
///
/// The `summary` is used verbatim in a failing test's message, so a failure
/// reads as *"wasm does not conform to R4.4: a guest past its memory budget
/// reports runtime.resource_exhausted…"* rather than as a bare assertion
/// mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Requirement {
    /// Stable id, e.g. `"R2.5"`. The suite has one test per id, and an
    /// enumeration test asserts that mapping is total and one-to-one.
    pub id: &'static str,
    /// The area of the contract this belongs to.
    pub group: &'static str,
    /// The guarantee, one line, stated in the present tense.
    pub summary: &'static str,
}

/// Contract A: the complete, enumerable list of conformance requirements.
///
/// Every backend must satisfy every entry. Adding a requirement here without a
/// corresponding test — or a test without an entry here — fails the suite's
/// `every_requirement_has_exactly_one_test` check.
pub const REQUIREMENTS: &[Requirement] = &[
    // --- caller isolation -------------------------------------------------
    Requirement {
        id: "R1.1",
        group: "caller_isolation",
        summary: "every instance observes exactly one caller for its whole lifetime",
    },
    Requirement {
        id: "R1.2",
        group: "caller_isolation",
        summary: "two callers are given two distinct instances",
    },
    Requirement {
        id: "R1.3",
        group: "caller_isolation",
        summary: "a handle's identity is fixed at spawn and cannot be altered by an invoke",
    },
    Requirement {
        id: "R1.4",
        group: "caller_isolation",
        summary: "a guest observes exactly the identity its handle was spawned with",
    },
    Requirement {
        id: "R1.5",
        group: "caller_isolation",
        summary: "interleaved invocations on two handles never cross identities",
    },
    Requirement {
        id: "R1.6",
        group: "caller_isolation",
        summary: "concurrent invocations on two handles never cross identities",
    },
    Requirement {
        id: "R1.7",
        group: "caller_isolation",
        summary: "the caller's role and groups come from the spawning identity",
    },
    Requirement {
        id: "R1.8",
        group: "caller_isolation",
        summary: "an anonymous caller arrives as anonymous, not as an empty user",
    },
    // --- lifecycle & errors --------------------------------------------------
    Requirement {
        id: "R2.1",
        group: "lifecycle",
        summary: "spawn, invoke, logs_stream and stop complete a round trip",
    },
    Requirement {
        id: "R2.2",
        group: "lifecycle",
        summary: "an invocation after the instance is gone reports runtime.stopped",
    },
    Requirement {
        id: "R2.3",
        group: "lifecycle",
        summary: "an absent artifact reports runtime.bundle_not_found",
    },
    Requirement {
        id: "R2.4",
        group: "lifecycle",
        summary: "a bundle ingest failure surfaces its bundle.* code verbatim from spawn",
    },
    Requirement {
        id: "R2.5",
        group: "lifecycle",
        summary: "a spent spawn deadline reports runtime.deadline_exceeded in the spawn phase",
    },
    Requirement {
        id: "R2.6",
        group: "lifecycle",
        summary: "a spent invoke deadline reports runtime.deadline_exceeded in the invoke phase",
    },
    // --- fault isolation --------------------------------------------------
    Requirement {
        id: "R3.1",
        group: "fault_isolation",
        summary: "a crashing guest reports runtime.trap with a non-empty message",
    },
    Requirement {
        id: "R3.2",
        group: "fault_isolation",
        summary: "one caller's crash does not stop another caller of the same bundle",
    },
    Requirement {
        id: "R3.3",
        group: "fault_isolation",
        summary: "a crash is reported as a trap, never as a budget failure",
    },
    // --- resource budgets --------------------------------------------------
    Requirement {
        id: "R4.1",
        group: "resource_budgets",
        summary: "a guest that never yields is preempted with runtime.deadline_exceeded in the \
                  invoke phase",
    },
    Requirement {
        id: "R4.2",
        group: "resource_budgets",
        summary: "the sealed CPU budget applies when the caller sets no deadline",
    },
    Requirement {
        id: "R4.3",
        group: "resource_budgets",
        summary: "a caller wall-clock deadline and the sealed CPU budget compose; whichever \
                  expires first ends the call",
    },
    Requirement {
        id: "R4.4",
        group: "resource_budgets",
        summary: "a guest past its memory budget reports runtime.resource_exhausted with limit \
                  \"memory\" and the sealed byte count as allowed",
    },
    Requirement {
        id: "R4.5",
        group: "resource_budgets",
        summary: "one guest exhausting its CPU quota does not affect a concurrent caller",
    },
    Requirement {
        id: "R4.6",
        group: "resource_budgets",
        summary: "more non-yielding guests than host workers are all preempted, and an unrelated \
                  caller is still served promptly",
    },
    // --- secrets --------------------------------------------------
    Requirement {
        id: "R5.1",
        group: "secrets",
        summary: "a secret granted by both the manifest and the store is readable",
    },
    Requirement {
        id: "R5.2",
        group: "secrets",
        summary: "a secret the store holds but the manifest does not declare is absent, and is \
                  indistinguishable from one that does not exist",
    },
    Requirement {
        id: "R5.3",
        group: "secrets",
        summary: "a secret the manifest declares but the store does not hold is absent, and is \
                  indistinguishable from one that does not exist",
    },
    // --- app data --------------------------------------------------
    Requirement {
        id: "R6.1",
        group: "app_data",
        summary: "get, set and delete round-trip, and deleting an unset key succeeds",
    },
    Requirement {
        id: "R6.2",
        group: "app_data",
        summary: "an empty value is stored and is distinct from absence",
    },
    Requirement {
        id: "R6.3",
        group: "app_data",
        summary: "two callers of one app read and write one shared namespace",
    },
    Requirement {
        id: "R6.4",
        group: "app_data",
        summary: "two apps have separate namespaces",
    },
    Requirement {
        id: "R6.5",
        group: "app_data",
        summary: "keys the documented grammar forbids are refused identically on every backend, \
                  including at the 128-byte boundary; accepting keys beyond the grammar is a \
                  backend's own choice",
    },
    Requirement {
        id: "R6.6",
        group: "app_data",
        summary: "a 1 MiB value is accepted; a value over the host's own limit is refused with \
                  value-too-large carrying a limit of at least 1 MiB",
    },
    // --- logs --------------------------------------------------
    Requirement {
        id: "R7.1",
        group: "logs",
        summary: "every log line from an invocation carries that invocation's request id",
    },
    Requirement {
        id: "R7.2",
        group: "logs",
        summary: "when guest output is dropped, at least one log line records that a drop occurred",
    },
];

/// The requirement with this id, or a panic naming the unknown id.
///
/// The suite calls this to attach a [`Requirement`] to each test; an unknown id
/// is a test referring to a requirement that was renamed or removed.
pub fn requirement(id: &str) -> &'static Requirement {
    REQUIREMENTS
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no conformance requirement with id {id:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn requirement_ids_are_unique() {
        let unique: BTreeSet<&str> = REQUIREMENTS.iter().map(|r| r.id).collect();
        assert_eq!(unique.len(), REQUIREMENTS.len(), "duplicate requirement id");
    }

    #[test]
    fn requirement_ids_are_well_formed() {
        for r in REQUIREMENTS {
            let (r_group, num) = r.id.split_at(1);
            assert_eq!(r_group, "R", "{:?} does not start with R", r.id);
            assert!(
                num.split_once('.').is_some_and(|(a, b)| !a.is_empty()
                    && !b.is_empty()
                    && a.chars().all(|c| c.is_ascii_digit())
                    && b.chars().all(|c| c.is_ascii_digit())),
                "{:?} is not R<n>.<n>",
                r.id
            );
            assert!(!r.summary.is_empty(), "{} has no summary", r.id);
            assert!(!r.group.is_empty(), "{} has no group", r.id);
        }
    }

    #[test]
    fn requirement_lookup_finds_every_id() {
        for r in REQUIREMENTS {
            assert_eq!(requirement(r.id).id, r.id);
        }
    }
}
