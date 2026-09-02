//! What a build weighs, and whether that is acceptable.
//!
//! # Why two types rather than one
//!
//! A component and a bundle are both measured in bytes and mean different
//! things. The component is what the runtime compiles, so its size drives cold
//! start; the bundle is the component plus every asset shipped beside it, and a
//! large one costs transfer and storage but not latency. A single `u64` passed
//! between the two would let an asset-heavy bundle trip a cold-start alarm, or
//! a bloated component hide under an assets budget.
//!
//! They are therefore separate types with no conversion between them, for the
//! same reason `SubjectId` and `OrgId` are separate: the compiler should reject
//! the mix-up rather than a reviewer catching it.
//!
//! # Where the thresholds came from
//!
//! Measured, not guessed. A template that answers an HTTP request and calls the
//! SDK is ~58 KB unstripped, ~53 KB stripped. Of that, `wasi:http` itself is
//! ~40 KB and unavoidable for anything serving a request; the clwd SDK adds
//! ~2 KB. The warn threshold sits at roughly 4.4x the measured template, which
//! is room to grow into rather than a line the first real app crosses.

use std::fmt;

/// The compiled component: what the runtime loads and compiles.
///
/// Compile time scales with this, and under scale-to-zero most requests hit a
/// cold sandbox, so this is the number that shows up as latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct ComponentSize(u64);

/// The packed bundle: the component plus every other member.
///
/// Costs transfer and storage. Does not affect cold start, because the runtime
/// compiles the component and not the assets beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct BundleSize(u64);

/// Warn here. Roughly 4.4x the measured template — growth, not a cliff.
const COMPONENT_WARN: u64 = 256 * 1024;
/// Fail here. A component this large is a packaging mistake, not a big app.
const COMPONENT_CEILING: u64 = 2 * 1024 * 1024;
/// Warn here. Assets are expected to dominate; this is where "expected" ends.
const BUNDLE_WARN: u64 = 4 * 1024 * 1024;
/// Fail here.
const BUNDLE_CEILING: u64 = 32 * 1024 * 1024;

/// What a measured size means for the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Under every threshold.
    Ok,
    /// Over the warn threshold. The build continues.
    Warn {
        /// The threshold that was crossed.
        threshold: u64,
    },
    /// Over the hard ceiling. The build fails.
    Exceeded {
        /// The ceiling that was crossed.
        ceiling: u64,
    },
}

impl Verdict {
    /// Whether this verdict should stop the build.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Exceeded { .. })
    }
}

/// Classify `bytes` against a warn threshold and a hard ceiling.
///
/// The ceiling is checked first so a size over both reports the ceiling: the
/// fatal fact is the useful one, and reporting a warning for something that is
/// about to fail the build would be actively misleading.
fn classify(bytes: u64, warn: u64, ceiling: u64) -> Verdict {
    if bytes > ceiling {
        Verdict::Exceeded { ceiling }
    } else if bytes > warn {
        Verdict::Warn { threshold: warn }
    } else {
        Verdict::Ok
    }
}

/// Render a byte count the way a person reads one.
///
/// Binary units, because that is what the thresholds are expressed in and a
/// number that disagrees with the limit it is being compared to is worse than
/// no number.
fn human(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

impl ComponentSize {
    /// Measure a component.
    #[must_use]
    pub fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The raw count.
    #[must_use]
    pub fn bytes(self) -> u64 {
        self.0
    }

    /// Classify against the component thresholds.
    #[must_use]
    pub fn verdict(self) -> Verdict {
        classify(self.0, COMPONENT_WARN, COMPONENT_CEILING)
    }
}

impl BundleSize {
    /// Measure a packed bundle.
    #[must_use]
    pub fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The raw count.
    #[must_use]
    pub fn bytes(self) -> u64 {
        self.0
    }

    /// Classify against the bundle thresholds.
    #[must_use]
    pub fn verdict(self) -> Verdict {
        classify(self.0, BUNDLE_WARN, BUNDLE_CEILING)
    }
}

impl fmt::Display for ComponentSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&human(self.0))
    }
}

impl fmt::Display for BundleSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&human(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured template, and the headroom above it. If this ever warns,
    /// either the template regressed badly or the threshold is wrong — and
    /// both are worth a failing test rather than a quiet warning in a log.
    #[test]
    fn the_measured_template_is_comfortably_under_the_warn_threshold() {
        let measured = ComponentSize::from_bytes(58_618);
        assert_eq!(measured.verdict(), Verdict::Ok);
        assert!(
            COMPONENT_WARN > measured.bytes() * 4,
            "the warn threshold ({COMPONENT_WARN}) should leave real room above \
             the measured template ({}), not sit just above it",
            measured.bytes()
        );
    }

    #[test]
    fn a_size_over_the_warn_threshold_warns_without_being_fatal() {
        let v = ComponentSize::from_bytes(COMPONENT_WARN + 1).verdict();
        assert_eq!(
            v,
            Verdict::Warn {
                threshold: COMPONENT_WARN
            }
        );
        assert!(!v.is_fatal());
    }

    #[test]
    fn a_size_over_the_ceiling_is_fatal() {
        let v = ComponentSize::from_bytes(COMPONENT_CEILING + 1).verdict();
        assert_eq!(
            v,
            Verdict::Exceeded {
                ceiling: COMPONENT_CEILING
            }
        );
        assert!(v.is_fatal());
    }

    /// Over both thresholds reports the ceiling, not the warning. A build about
    /// to fail should say so rather than emit a warning that reads as survivable.
    #[test]
    fn over_both_thresholds_reports_the_ceiling_not_the_warning() {
        let v = ComponentSize::from_bytes(COMPONENT_CEILING * 2).verdict();
        assert!(v.is_fatal(), "expected fatal, got {v:?}");
        assert_eq!(
            v,
            Verdict::Exceeded {
                ceiling: COMPONENT_CEILING
            }
        );
    }

    /// Exactly on a threshold is allowed. `>` not `>=`, so a build sitting on a
    /// documented limit passes rather than failing on an off-by-one nobody can
    /// see from the number alone.
    #[test]
    fn a_size_exactly_on_a_threshold_is_allowed() {
        assert_eq!(
            ComponentSize::from_bytes(COMPONENT_WARN).verdict(),
            Verdict::Ok
        );
        assert!(
            !ComponentSize::from_bytes(COMPONENT_CEILING)
                .verdict()
                .is_fatal()
        );
    }

    /// The two types must not share thresholds. A bundle carrying assets is
    /// expected to be far larger than any component, and judging it by the
    /// component's limits would fail every app that ships a stylesheet.
    #[test]
    fn a_bundle_is_not_judged_by_the_component_thresholds() {
        let assets = 8 * 1024 * 1024;
        assert_eq!(
            BundleSize::from_bytes(assets).verdict(),
            Verdict::Warn {
                threshold: BUNDLE_WARN
            },
            "8 MiB of assets is a warning for a bundle, not a failure"
        );
        assert!(
            ComponentSize::from_bytes(assets).verdict().is_fatal(),
            "the same byte count must be fatal for a component — otherwise the \
             two types are sharing limits and the split is decorative"
        );
    }

    #[test]
    fn sizes_render_in_the_units_the_thresholds_use() {
        assert_eq!(ComponentSize::from_bytes(512).to_string(), "512 B");
        assert_eq!(ComponentSize::from_bytes(58_618).to_string(), "57.2 KiB");
        assert_eq!(
            BundleSize::from_bytes(5 * 1024 * 1024).to_string(),
            "5.0 MiB"
        );
    }
}
