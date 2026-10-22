//! `clwd init`, and the guard that it still produces the template CI builds.
//!
//! # Why the round trip
//!
//! Templates are real projects that CI compiles, and `init` embeds a copy of
//! them in the binary. That is two paths to the same bytes, and #72 showed what
//! happens when nobody checks a second path: the guest fixture rotted because
//! the conformance suite ran against a committed artifact rather than the
//! source.
//!
//! Here the specific failure is an embedded file list that silently omits a
//! file. The template still builds in CI — the file is on disk — and `init`
//! produces a project missing it. Nothing else would notice.
//!
//! So the test undoes the substitutions and compares against the committed
//! template, file for file and byte for byte.

use std::path::{Path, PathBuf};
use std::process::Command;

use clwd_cli::commands::init;

/// The repository's `templates/` directory.
fn templates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .canonicalize()
        .expect("templates/ is committed")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clwd-init-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every file `init` writes matches the committed template it came from, once
/// the substitutions are undone.
///
/// **This is the guard against an embedded list that omits a file.** A missing
/// entry shows up as a file present on disk and absent from the scaffold.
#[test]
fn init_output_matches_the_template_it_copied() {
    for (template, _) in init::list() {
        let scratch = Scratch::new(&format!("roundtrip-{template}"));
        // A name different from the template's, so a substitution that silently
        // does nothing cannot pass by coincidence.
        let name = "scaffolded-app";

        let made = init::execute(&scratch.0, name, Some(template), None)
            .unwrap_or_else(|e| panic!("init {template} failed: {e}"));

        let source = templates_dir().join(template);

        // Same set of files, in both directions.
        let mut on_disk: Vec<String> = walk(&source, &source);
        on_disk.sort();
        let mut scaffolded = made.files.clone();
        scaffolded.sort();
        assert_eq!(
            scaffolded, on_disk,
            "the embedded file list for {template} does not match templates/{template}/ — \
             a file was added or removed without updating TEMPLATES"
        );

        // Same bytes, once the substitutions are undone.
        for relative in &on_disk {
            let original = std::fs::read_to_string(source.join(relative))
                .unwrap_or_else(|e| panic!("reading templates/{template}/{relative}: {e}"));
            let written = std::fs::read_to_string(made.project.join(relative))
                .unwrap_or_else(|e| panic!("reading the scaffolded {relative}: {e}"));

            let restored = undo_substitutions(&written, template, name);
            assert_eq!(
                restored, original,
                "{template}/{relative} differs from the committed template by more than \
                 the substitutions"
            );
        }
    }
}

/// Reverse what `init` rewrote, so the result should equal the committed file.
fn undo_substitutions(written: &str, template: &str, name: &str) -> String {
    let mut out = written.to_owned();
    // The SDK path is rewritten to an absolute one; put the relative form back.
    // Matched by shape rather than by value, because the absolute path depends
    // on where this checkout is.
    let mut rebuilt = String::new();
    for line in out.lines() {
        if line.trim_start().starts_with("clwd-sdk = { path = ") {
            rebuilt.push_str("clwd-sdk = { path = \"../../crates/sdk\" }");
        } else {
            rebuilt.push_str(line);
        }
        rebuilt.push('\n');
    }
    out = rebuilt;
    out.replace(
        &format!("name = \"{name}\""),
        &format!("name = \"{template}\""),
    )
    .replace(
        &format!("host_label = \"{name}\""),
        &format!("host_label = \"{template}\""),
    )
}

/// Every file under `dir`, as paths relative to `root`, with `/` separators.
fn walk(root: &Path, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Build output is not part of a template. `Cargo.lock` is in that
        // category here and is gitignored: it pins clwd-sdk through the
        // relative path the repository uses, and `init` rewrites that path, so
        // a shipped lockfile would describe a resolution the scaffolded project
        // does not have.
        //
        // Named individually rather than by a pattern, so deciding that
        // templates SHOULD ship a lockfile is one edit here and one in
        // TEMPLATES, not a silent change in what this test covers.
        if path
            .file_name()
            .is_some_and(|n| n == "target" || n == "Cargo.lock")
        {
            continue;
        }
        if path.is_dir() {
            out.extend(walk(root, &path));
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    out
}

/// The name lands in both manifests, which is the whole substitution.
#[test]
fn the_project_name_reaches_both_manifests() {
    let scratch = Scratch::new("names");
    let made = init::execute(&scratch.0, "my-app", Some("minimal"), None).expect("init");

    let cargo = std::fs::read_to_string(made.project.join("Cargo.toml")).expect("Cargo.toml");
    let clwd = std::fs::read_to_string(made.project.join("clwd.toml")).expect("clwd.toml");

    assert!(cargo.contains("name = \"my-app\""), "{cargo}");
    assert!(clwd.contains("name = \"my-app\""), "{clwd}");
    assert!(clwd.contains("host_label = \"my-app\""), "{clwd}");
    assert!(
        !cargo.contains("name = \"minimal\""),
        "the template's own name survived: {cargo}"
    );
}

/// A name that is not a DNS label is not made into a host label: the line is
/// left commented, next to the sentence saying a label is permanent, and
/// `clwd deploy` refuses until the author picks one.
#[test]
fn a_name_that_is_not_a_label_leaves_the_host_label_for_the_author() {
    let scratch = Scratch::new("nolabel");
    let made = init::execute(&scratch.0, "my_app", Some("minimal"), None).expect("init");
    let clwd = std::fs::read_to_string(made.project.join("clwd.toml")).expect("clwd.toml");

    assert!(clwd.contains("# host_label = \"your-app\""), "{clwd}");
    assert!(clwd.contains("Permanent once deployed"), "{clwd}");
    let source = clwd_cli::build::Source::parse(&clwd).expect("still valid");
    assert_eq!(source.host_label(), None, "{clwd}");
}

/// `init` refuses to write into something that exists, rather than merging into
/// it or overwriting a file someone cares about.
#[test]
fn init_refuses_an_existing_directory() {
    let scratch = Scratch::new("existing");
    std::fs::create_dir_all(scratch.0.join("taken")).expect("mkdir");

    let err = init::execute(&scratch.0, "taken", None, None).expect_err("must refuse");
    assert_eq!(err.code(), "cli.invalid_argument");
    assert!(err.to_string().contains("already exists"), "{err}");
}

/// The name rule is the intersection of two grammars, and the message says so —
/// otherwise someone widens it to match app names and breaks crate generation.
#[test]
fn an_invalid_name_is_refused_with_the_rule() {
    let scratch = Scratch::new("badname");
    let err = init::execute(&scratch.0, "my.app", None, None).expect_err("a dot must be refused");
    let msg = err.to_string();
    assert!(msg.contains("cargo"), "why: {msg}");
    assert!(msg.contains("a-z"), "the rule: {msg}");
}

/// The end-to-end promise: what `init` writes, `build` builds and `run` runs.
///
/// Not a unit test of three functions — the real binary, on a real scaffold, as
/// an author would.
#[test]
fn init_then_build_then_run_works_end_to_end() {
    let scratch = Scratch::new("e2e");
    let made = init::execute(&scratch.0, "e2e-app", Some("minimal"), None).expect("init");

    let bundle = scratch.0.join("app.clwd");
    let build = Command::new(env!("CARGO_BIN_EXE_clwd"))
        .args(["build"])
        .arg(&made.project)
        .arg("--out")
        .arg(&bundle)
        .output()
        .expect("clwd build runs");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(env!("CARGO_BIN_EXE_clwd"))
        .args(["run"])
        .arg(&bundle)
        .args(["--path", "/", "--project"])
        .arg(&made.project)
        .output()
        .expect("clwd run runs");
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "hello",
        "the scaffolded app did not answer"
    );
}
