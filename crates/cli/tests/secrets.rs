//! `clwd secrets` through the real binary, and the leak sweep.
//!
//! # Why the positive control comes first
//!
//! A grep for a value that was never stored passes trivially. Every assertion
//! below that a secret is *absent* from something is therefore preceded by an
//! assertion that it is *present* where it should be — retrievable from the
//! store, byte for byte. Without that half, this file would keep passing after
//! a change that broke `set` entirely, and would keep passing if the sentinel
//! were misspelled.
//!
//! This project has twice shipped a test that checked nothing: a drift check
//! that could not see stale data, and an optimiser comparison that skipped
//! itself in the one environment that mattered. The control is the cheap
//! defence against being wrong a third time.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Distinctive enough that finding it anywhere is unambiguous, and not a
/// substring of anything the CLI prints on its own.
const SENTINEL: &str = "zzq-sentinel-value-8f3a1c-do-not-log";

struct Project {
    root: PathBuf,
    config: PathBuf,
}

impl Project {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("clwd-secrets-it-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("proj");
        std::fs::create_dir_all(&dir).expect("project dir");
        std::fs::write(
            dir.join("clwd.toml"),
            "[app]\nname = \"leaktest\"\n\n[capabilities]\nsecrets = [\"api-token\"]\n",
        )
        .expect("manifest");
        Self {
            config: root.join("cfg"),
            root,
        }
    }

    fn dir(&self) -> PathBuf {
        self.root.join("proj")
    }

    fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_clwd"));
        cmd.args(args)
            .arg("--project")
            .arg(self.dir())
            .env("XDG_CONFIG_HOME", &self.config)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().expect("the clwd binary is a test dependency");
        if let Some(bytes) = stdin {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .expect("piped")
                .write_all(bytes)
                .expect("write stdin");
        }
        child.wait_with_output().expect("output")
    }

    fn store_path(&self) -> PathBuf {
        self.config.join("clwd").join("secrets.json")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The leak sweep. **The first assertion is the control**: if the secret is not
/// actually stored and retrievable, every absence below proves nothing.
#[test]
fn a_stored_secret_never_appears_where_it_should_not() {
    let p = Project::new("leak");

    let set = p.run(&["secrets", "set", "api-token"], Some(SENTINEL.as_bytes()));
    assert!(set.status.success(), "set failed: {}", text(&set.stderr));

    // ---- POSITIVE CONTROL ----
    // Without this, every assertion that follows would pass on an empty store.
    let got = p.run(&["secrets", "get", "api-token"], None);
    assert!(got.status.success(), "get failed: {}", text(&got.stderr));
    assert_eq!(
        got.stdout,
        SENTINEL.as_bytes(),
        "the control failed: the secret did not round-trip, so the absence \
         assertions below would be vacuous"
    );

    // ---- and now the absences ----
    assert!(
        !text(&set.stdout).contains(SENTINEL) && !text(&set.stderr).contains(SENTINEL),
        "`set` echoed the value it was given"
    );

    for args in [vec!["secrets", "list"], vec!["--json", "secrets", "list"]] {
        let out = p.run(&args, None);
        assert!(
            !text(&out.stdout).contains(SENTINEL),
            "for {args:?}: the value reached stdout"
        );
        assert!(
            !text(&out.stderr).contains(SENTINEL),
            "for {args:?}: the value reached stderr"
        );
    }

    // The store itself holds it base64-encoded, so the plaintext must not be
    // findable there either — a reader grepping for a leaked token would search
    // for the plaintext.
    let raw = std::fs::read(p.store_path()).expect("the store exists");
    assert!(
        !text(&raw).contains(SENTINEL),
        "the store holds the value in plaintext"
    );
}

/// A failure path is where a value most easily escapes: an error message that
/// helpfully includes what it was given.
#[test]
fn no_error_message_echoes_a_value() {
    let p = Project::new("errors");
    // An invalid name, with a valid value on stdin. The name is rejected after
    // stdin exists, which is exactly when a careless message would quote it.
    for args in [
        vec!["secrets", "set", "../bad-name"],
        vec!["--json", "secrets", "set", "../bad-name"],
    ] {
        let out = p.run(&args, Some(SENTINEL.as_bytes()));
        assert!(!out.status.success(), "for {args:?}: expected a failure");
        assert!(
            !text(&out.stdout).contains(SENTINEL) && !text(&out.stderr).contains(SENTINEL),
            "for {args:?}: the value appeared in an error"
        );
    }
}

/// Secrets are scoped to a declared app name, and the CLI says so rather than
/// guessing from the directory.
#[test]
fn an_app_without_a_declared_name_is_told_what_to_add() {
    let p = Project::new("noname");
    std::fs::write(p.dir().join("clwd.toml"), "[capabilities]\nsecrets = []\n")
        .expect("rewrite manifest");

    let out = p.run(&["secrets", "list"], None);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(err.contains("[app]"), "the fix should be shown: {err}");
    assert!(err.contains("name"), "the key should be named: {err}");
}

/// Operator-facing, unlike the guest broker: a missing secret says so.
#[test]
fn getting_a_secret_that_is_not_there_says_so() {
    let p = Project::new("missing");
    let out = p.run(&["secrets", "get", "absent"], None);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("no secret named"),
        "got: {}",
        text(&out.stderr)
    );
}

/// Removing what is not there succeeds, so a teardown script need not check.
#[test]
fn removing_an_absent_secret_succeeds() {
    let p = Project::new("rm");
    let out = p.run(&["secrets", "rm", "absent"], None);
    assert!(out.status.success(), "{}", text(&out.stderr));
}

/// An empty pipe is a mistake worth naming rather than a zero-byte secret.
#[test]
fn setting_from_an_empty_pipe_is_refused_with_the_shape_of_the_command() {
    let p = Project::new("empty");
    let out = p.run(&["secrets", "set", "api-token"], Some(b""));
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(err.contains('<'), "the invocation should be shown: {err}");
}

/// The store is created private, checked on the artifact the CLI actually
/// wrote rather than on the code that wrote it.
#[cfg(unix)]
#[test]
fn the_store_the_binary_creates_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let p = Project::new("modes");
    let out = p.run(&["secrets", "set", "api-token"], Some(b"value"));
    assert!(out.status.success(), "{}", text(&out.stderr));

    let mode = std::fs::metadata(p.store_path())
        .expect("store exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "store is {mode:o}");
}

/// The value survives as bytes, which is what the WIT's `list<u8>` requires.
#[test]
fn a_non_utf8_value_survives_the_round_trip_through_the_binary() {
    let p = Project::new("bytes");
    let raw: &[u8] = &[0xff, 0x00, 0xfe, 0x80, 0x0a];
    let set = p.run(&["secrets", "set", "api-token"], Some(raw));
    assert!(set.status.success(), "{}", text(&set.stderr));

    let got = p.run(&["secrets", "get", "api-token"], None);
    assert_eq!(
        got.stdout, raw,
        "bytes did not survive; a key or certificate would be corrupted"
    );
}

/// Sanity on the fixture itself: the sentinel must not already be present in
/// the repository, or the sweep above would be asserting against noise.
#[test]
fn the_sentinel_is_not_something_the_repository_already_contains() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let src = root.join("crates");
    let mut found = Vec::new();
    visit(&src, &mut found);
    assert!(
        found.is_empty(),
        "the sentinel appears in {found:?}, so the leak sweep cannot distinguish \
         a leak from an existing occurrence"
    );
}

fn visit(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            visit(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != "secrets.rs")
            && std::fs::read_to_string(&path).is_ok_and(|t| t.contains(SENTINEL))
        {
            found.push(path);
        }
    }
}

/// The injection set is the intersection of what the manifest declares and what
/// the store holds — conformance R5.2 and R5.3, asserted on the *local* path.
///
/// The conformance suite covers this against `MemorySecretStore`. That is a
/// different store, so it proves nothing about this one; a bug that let the
/// file store return an undeclared secret would leave the suite green. Both
/// secrets below live in the same namespace, so the manifest is the only thing
/// separating them.
#[cfg(feature = "run")]
#[test]
fn a_guest_sees_only_the_secrets_its_manifest_declared() {
    use clwd_bundle::{Capabilities, Limits, Runtime, build};

    /// The reference guest, which answers `/secret/{name}` with the value or
    /// the literal `absent`.
    const GUEST: &[u8] = include_bytes!("../../runtime/tests/fixtures/app.wasm");

    let p = Project::new("intersection");

    // Held: both. Declared: one.
    let set = p.run(&["secrets", "set", "api-token"], Some(b"DECLARED-VALUE"));
    assert!(set.status.success(), "{}", text(&set.stderr));
    let set = p.run(
        &["secrets", "set", "other-token"],
        Some(b"UNDECLARED-VALUE"),
    );
    assert!(set.status.success(), "{}", text(&set.stderr));

    let encoded = build(
        Runtime::Wasm,
        "app.wasm",
        Capabilities {
            version: 1,
            egress: Vec::new(),
            // `api-token` only. `other-token` is held but not declared.
            secrets: vec!["api-token".to_owned()],
            storage_namespaces: Vec::new(),
            limits: Limits {
                cpu_ms: 5_000,
                memory_mb: 128,
            },
        },
        vec![clwd_bundle::File {
            path: "app.wasm".to_owned(),
            data: GUEST.to_vec(),
            executable: false,
        }],
    )
    .expect("the reference guest makes a valid bundle");
    let bundle = p.root.join("app.clwd");
    std::fs::write(&bundle, encoded.bytes).expect("write bundle");
    let bundle = bundle.to_string_lossy().into_owned();

    // ---- POSITIVE CONTROL: the declared one really does arrive ----
    let declared = p.run(&["run", &bundle, "--path", "/secret/api-token"], None);
    assert!(declared.status.success(), "{}", text(&declared.stderr));
    assert_eq!(
        text(&declared.stdout).trim(),
        "DECLARED-VALUE",
        "the declared secret did not reach the guest, so the absence below \
         would prove nothing"
    );

    // ---- and the undeclared one does not ----
    let undeclared = p.run(&["run", &bundle, "--path", "/secret/other-token"], None);
    assert!(undeclared.status.success(), "{}", text(&undeclared.stderr));
    assert_eq!(
        text(&undeclared.stdout).trim(),
        "absent",
        "a secret the manifest did not declare reached the guest"
    );
    assert!(
        !text(&undeclared.stdout).contains("UNDECLARED-VALUE"),
        "the undeclared value leaked into the response"
    );
}
