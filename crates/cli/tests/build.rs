//! `clwd build` end to end: a real project, compiled, bundled, and then handed
//! to the same code path `clwd run` uses to accept a bundle.
//!
//! The project compiled here is the guest fixture the runtime tests already
//! carry. Using it rather than a purpose-made one is deliberate: it is a real
//! component with a real WIT world, it is excluded from the workspace and has
//! its own lockfile, and it is already the artifact the runtime's own
//! conformance suite runs. A fixture invented for this test could drift into
//! something that builds here and nowhere else.
//!
//! These tests invoke `cargo` and therefore take tens of seconds on a cold
//! target directory. That is the cost of asserting the thing the issue actually
//! promises — a bundle produced by `build` that `run` accepts — rather than
//! asserting the two halves separately and hoping they meet.

use std::path::{Path, PathBuf};

use clwd_cli::build::{self, Options};

/// The guest fixture: a real `wasm32-wasip2` component with a WIT world.
fn fixture_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../runtime/tests/fixtures/guest")
        .canonicalize()
        .expect("the guest fixture is committed and should resolve")
}

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("clwd-build-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn build_fixture(out: PathBuf) -> build::Built {
    let mut options = Options::new(fixture_project());
    options.out = Some(out);
    build::run(&options, None).expect("the guest fixture should build")
}

/// The headline promise of the issue: what `build` writes, `run` accepts.
///
/// Acceptance is asserted through `clwd_bundle::decode`, which is the check
/// `clwd run` performs before it will spawn anything — see `read_bundle` in the
/// run command. A bundle that decodes is one `run` will take.
#[test]
fn build_produces_a_bundle_that_run_accepts() {
    let scratch = Scratch::new("accepts");
    let out = scratch.join("app.clwd");
    let built = build_fixture(out.clone());

    assert!(out.is_file(), "no bundle at {}", out.display());
    assert_eq!(built.bundle, out);

    let bytes = std::fs::read(&out).expect("the bundle reads back");
    let bundle =
        clwd_bundle::decode(&bytes, None).expect("run must be able to decode what build wrote");

    assert_eq!(
        bundle.manifest.runtime,
        clwd_bundle::Runtime::Wasm,
        "the fixture is a wasm component"
    );
    assert!(
        bundle
            .manifest
            .members
            .iter()
            .any(|m| m.path == bundle.manifest.entrypoint),
        "the entrypoint must name a real member, or the bundle is unrunnable"
    );
    assert_eq!(
        built.digest,
        bundle.digest.to_string(),
        "the digest reported to the caller must be the bundle's own"
    );
}

/// Deny-by-default, asserted on the sealed artifact rather than on the code
/// that produced it. A project declaring nothing must ship a manifest granting
/// nothing — as three explicit empty arrays, per spec §5.3.
#[test]
fn a_project_declaring_nothing_ships_a_bundle_granting_nothing() {
    let scratch = Scratch::new("deny");
    let built = build_fixture(scratch.join("app.clwd"));

    let bytes = std::fs::read(&built.bundle).expect("reads");
    let caps = clwd_bundle::decode(&bytes, None)
        .expect("decodes")
        .manifest
        .capabilities;

    assert!(caps.egress.is_empty(), "egress: {:?}", caps.egress);
    assert!(caps.secrets.is_empty(), "secrets: {:?}", caps.secrets);
    assert!(caps.storage_namespaces.is_empty());

    // Visible, not merely absent: the build must say what it granted even when
    // that is nothing, or an author cannot tell deny-by-default from a bug.
    assert!(
        built.capabilities.iter().any(|l| l == "egress: none"),
        "the sealed set must be reported in full, got {:?}",
        built.capabilities
    );
}

/// The incremental claim, asserted rather than timed. A second build with no
/// source change must not rewrite the bundle.
///
/// Timing this would measure the machine; asserting the flag measures the
/// decision. `unchanged` is set from a byte comparison against the file already
/// on disk, so a true value means the bytes were identical, not that a clock
/// was fast.
#[test]
fn a_second_build_with_no_change_leaves_the_bundle_alone() {
    let scratch = Scratch::new("incremental");
    let out = scratch.join("app.clwd");

    let first = build_fixture(out.clone());
    assert!(
        !first.unchanged,
        "the first build writes the bundle; there was nothing there to match"
    );

    let second = build_fixture(out.clone());
    assert!(
        second.unchanged,
        "an unchanged source tree must produce an unchanged bundle"
    );
    assert_eq!(
        first.digest, second.digest,
        "the same input must produce the same content address"
    );
}

/// `--no-optimize` is observable in the artifact, not just in the flags. If it
/// were not, the flag would be decorative.
///
/// Unconditional, which is why CI installs `wasm-tools`. An earlier version
/// skipped the comparison when the tool was absent — and since CI is exactly
/// where it is absent by default, that is a test which passes everywhere and
/// checks nothing in the only place it matters. The absent-tool path is covered
/// separately, and purely, in `build::toolchain`'s own tests.
#[test]
fn skipping_the_optimiser_produces_a_larger_component() {
    let scratch = Scratch::new("optimize");

    let mut plain = Options::new(fixture_project());
    plain.out = Some(scratch.join("stripped.clwd"));
    let stripped = build::run(&plain, None).expect("builds");

    let mut raw = Options::new(fixture_project());
    raw.out = Some(scratch.join("unstripped.clwd"));
    raw.optimize = false;
    let unstripped = build::run(&raw, None).expect("builds");

    assert!(
        stripped.stripped,
        "wasm-tools is required for this test and CI installs it; \
         install it locally with `cargo install wasm-tools`. Warnings: {:?}",
        stripped.warnings
    );
    assert!(
        !unstripped.stripped,
        "--no-optimize must actually skip the optimiser"
    );
    assert!(
        stripped.component_size < unstripped.component_size,
        "stripping must shrink the component: {} vs {}",
        stripped.component_size,
        unstripped.component_size
    );
}
