//! The published error-code table, and the checks that keep it honest.
//!
//! `docs/error-codes.md` is generated from here, not written by hand. A
//! hand-kept table drifts the first time someone adds a variant, and the whole
//! value of a published table is that a reader can trust it.
//!
//! # Regenerating
//!
//! ```text
//! just error-table   # rewrite docs/error-codes.md after a deliberate change
//! ```
//!
//! Staleness is caught by `docs_match_the_code` below, which `cargo test
//! --workspace` runs — so `just check` already fails on a stale page without a
//! separate recipe.
//!
//! # What the tests here enforce
//!
//! - **Completeness.** Every code in every catalogue has a row, and every row
//!   names a real code. Adding a variant without documenting it fails.
//! - **Compatibility.** The committed `docs/error-codes.md` is the snapshot a
//!   rename is compared against. `CODES` and its exhaustiveness test both
//!   regenerate from the same macro literal, so they move together and cannot
//!   catch a rename — verified by mutation: renaming `cli.not_found` to
//!   `cli.missing` passes every test in `clwd-cli`. The committed file is the
//!   external anchor that does not move.
//! - **Usefulness.** Meaning and recovery are non-empty, and a recovery is not
//!   the code name in longer words.
//!
//! # Why the prose lives here and not beside each variant
//!
//! "Documentation lives next to the thing it documents" is the usual instinct,
//! and someone will want to move these 50 entries onto their `error.rs`
//! variants. Resist it. Completeness is enforced either way — a variant with no
//! row fails `table_covers_every_code_and_invents_none` — so the only thing at
//! stake is whether the writing is any good. The 38 `bundle.*` entries are one
//! narrative about one artifact: they share vocabulary, they build on each
//! other, and several deliberately end in the same sentence because the same
//! action fixes them. Written and edited in one file, that reads as prose.
//! Split across macro invocations, it gets written one variant at a time by
//! someone in a hurry, and reads like it.
//!
//! # Audience
//!
//! Two readers, neither of whom wrote this: a non-engineer running small
//! software, and an agent recovering without a human. So `meaning` explains the
//! situation in plain words — no tar headers, no canonical byte streams, no
//! USTAR prefix fields — and `recovery` is an action with a command in it, not
//! a description of the problem restated as advice.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// What a caller should *do*, as a value rather than prose.
///
/// `retryable: bool` in the envelope is binary and currently uniformly false,
/// so it gives an agent no signal. This is the richer axis.
///
/// **Table only, deliberately not in the envelope.** Adding a field to the wire
/// format is a compatibility change `platform#2` would have to mirror, and that
/// deserves its own decision rather than arriving as a side effect of writing
/// documentation. If these categories prove out in practice, promoting one into
/// the envelope later is additive and breaks nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    /// Transient. The same command may succeed unchanged.
    Retry,
    /// The command line, a file, or a manifest value is wrong. Change it.
    FixInput,
    /// The caller is not allowed to do this, or a credential is missing.
    ///
    /// Carried by `cli.toolchain_missing` since #28: a tool the build needs is
    /// absent from the machine, which the caller fixes by installing it rather
    /// than by changing the project. Nothing in the CLI authenticates yet, so
    /// the credential half of this category is still unused.
    FixPermissions,
    /// The artifact is damaged or stale. Build it again.
    Rebuild,
    /// A defect in clwd. Nothing the caller can change will help.
    ReportBug,
}

impl Recovery {
    fn as_str(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::FixInput => "fix_input",
            Self::FixPermissions => "fix_permissions",
            Self::Rebuild => "rebuild",
            Self::ReportBug => "report_bug",
        }
    }
}

/// Whether a CLI user can hit this code today.
///
/// **Derived, not declared per row.** Reachability is a property of whether the
/// CLI delegates a namespace at all, so it is one fact per namespace rather than
/// 49 hand-kept flags. Writing it per row meant a change like #25 — which made
/// 44 codes reachable at once — had to be applied 44 times, and the drift check
/// cannot catch a half-finished pass: it compares the generated page against the
/// committed one, and both come from the same hand-written data. Deriving it
/// makes the inconsistent state unrepresentable instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Reachable from the CLI today.
    Now,
    /// Not reachable yet, and the issue that changes that.
    Soon(&'static str),
}

impl Reach {
    /// Reachability for a code, from its namespace.
    ///
    /// `cli.*` is always reachable — the CLI owns it. `runtime.*` arrives through
    /// `CliError::Runtime`, the delegating variant behind the `run` feature, so
    /// it is reachable exactly when a command drives the sandbox; `clwd run`
    /// (#25) is that command. `bundle.*` arrived the same way until #28 gave
    /// `CliError` a direct `Bundle` variant, so it is now reachable with the
    /// feature off too — `clwd build` writes a bundle without a sandbox.
    fn of(code: &str) -> Self {
        match code.split_once('.').expect("codes are namespaced").0 {
            "cli" => Self::Now,
            // Before #25 nothing constructed a delegating variant, so every one
            // of these was unreachable.
            "runtime" | "bundle" => Self::Now,
            // `net.*` is owned by the CLI and reachable since #26: `clwd publish`
            // makes an HTTP call, and a failure with no response is `net.*`.
            "net" => Self::Now,
            other => panic!("no reachability rule for the {other:?} namespace"),
        }
    }
}

struct Row {
    code: &'static str,
    meaning: &'static str,
    recovery: &'static str,
    action: Recovery,
}

/// One row per code. Grouped by the subsystem that decides the code, because
/// the prefix names that subsystem and a reader tracing an error backwards
/// starts from the prefix.
const TABLE: &[Row] = &[
    // ---- cli.* — decided by clwd-cli, all reachable today ----
    Row {
        code: "cli.invalid_argument",
        meaning: "The command line was not accepted. Either an option was spelled in a way \
                  clwd does not recognise, or a value was the right shape but not allowed \
                  for that command.",
        recovery: "Run `clwd <command> --help` to see the accepted options and values, then \
                   run the command again with the corrected one.",
        action: Recovery::FixInput,
    },
    Row {
        code: "cli.not_found",
        meaning: "Something the command referred to by name — an app, a secret, a \
                  bundle — does not exist.",
        recovery: "Check the name for typos. If it looks right, the thing may have been \
                   deleted or belong to a different organisation than the one you are \
                   signed in to.",
        action: Recovery::FixInput,
    },
    Row {
        code: "cli.not_implemented",
        meaning: "The command exists and is spelled correctly, but has not been built yet. \
                  It appears in `--help` so that the surface is honest about what is \
                  coming.",
        recovery: "There is nothing to fix locally. Track the command's issue at \
                   https://github.com/clwdrun/clwd/issues to see when it lands.",
        action: Recovery::ReportBug,
    },
    Row {
        code: "cli.config",
        meaning: "clwd could not read its own settings for this machine. Either the \
                  config file is not where clwd expects it, or it is there but cannot be \
                  parsed, or two settings in it contradict each other. This usually \
                  follows a hand-edit, an interrupted `clwd init`, or a config written by \
                  a different version of clwd.",
        recovery: "The message says which setting or file was at fault. If a file is \
                   present, correct it; deleting it is always safe, because clwd falls \
                   back to its defaults and writes a fresh one. If the message names a \
                   setting rather than a file, run `clwd init` to regenerate the config.",
        action: Recovery::FixInput,
    },
    Row {
        code: "cli.manifest_invalid",
        meaning: "The `clwd.toml` in the project being built says something clwd cannot \
                  act on. Common causes: a key spelled wrongly, a capability granted twice, \
                  a grant that names nothing, a runtime clwd does not have, or the project's \
                  own `clwd.toml` swept into the files being bundled — where it would collide \
                  with the one the build writes. Unknown keys are refused rather than \
                  skipped: most keys here grant a capability, and one quietly ignored is a \
                  permission the author believes they have and does not.",
        recovery: "The message names the offending key or file and what is wrong with it. \
                   Correct that line. Do not delete the file to start over — it holds the \
                   app's capability grants, and an empty one grants nothing at all.",
        action: Recovery::FixInput,
    },
    Row {
        code: "cli.toolchain_missing",
        meaning: "A program the build needs is not installed, or is installed but will not \
                  run on this machine. clwd shells out to a small number of external tools \
                  and does not vendor them, so a fresh machine can be missing one.",
        recovery: "The message names the tool and the exact command that installs it. Run \
                   that command, then run the build again. Nothing in the project needs to \
                   change.",
        action: Recovery::FixPermissions,
    },
    Row {
        code: "cli.build_failed",
        meaning: "A step of the build ran and reported failure. The commonest cause is that \
                  the app does not compile; the compiler's own output says why. This code \
                  means the step failed, not that clwd malfunctioned.",
        recovery: "Read the output printed above the error — it comes from the step that \
                   failed and is more specific than anything clwd can add. Fix what it \
                   reports, then build again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "cli.internal",
        meaning: "clwd hit a problem it does not have a specific explanation for. This is \
                  always a defect in clwd rather than a mistake by the caller.",
        recovery: "Nothing the caller can change will help. Report it at \
                   https://github.com/clwdrun/clwd/issues with the command that produced \
                   it.",
        action: Recovery::ReportBug,
    },
    // ---- net.* — decided by clwd-cli, for a call that reached no platform ----
    Row {
        code: "net.unreachable",
        meaning: "A command that talks to a platform — `clwd publish` — could not reach one. \
                  The connection was refused, timed out, or failed to establish a secure \
                  channel, so no platform ever saw the request and none could say what was \
                  wrong. This is the network between you and the platform, not the platform \
                  itself.",
        recovery: "Check the platform URL (the `--platform` flag or `CLWD_PLATFORM_URL`) and \
                   that you have a working connection, then run the command again. A publish \
                   is safe to repeat: the platform records a version once per bundle, so a \
                   retry never creates a duplicate.",
        action: Recovery::Retry,
    },
    // ---- runtime.* — decided by clwd-runtime ----
    Row {
        code: "runtime.bundle_not_found",
        meaning: "The app was asked for by its content address, but no stored copy with \
                  that address exists. The address is well formed; nothing is stored under \
                  it.",
        recovery: "Publish the app first with `clwd publish`, or check that the address \
                   came from the build you meant.",
        action: Recovery::FixInput,
    },
    Row {
        code: "runtime.deadline_exceeded",
        meaning: "The app ran longer than it was allowed to. The message says whether the \
                  limit was hit while the app was starting up or while it was handling a \
                  request.",
        recovery: "Retrying may work if the app was briefly slow. If it happens every \
                   time, the app is doing too much work for its time budget — raise \
                   `capabilities.limits.cpu_ms` in `clwd.toml` and rebuild.",
        action: Recovery::Retry,
    },
    Row {
        code: "runtime.stopped",
        meaning: "The app was running but is no longer. It exited on its own, crashed, or \
                  was reclaimed by the host.",
        recovery: "Start it again. If it stops repeatedly, `clwd logs` will show what the \
                   app printed before it went away.",
        action: Recovery::Retry,
    },
    Row {
        code: "runtime.resource_exhausted",
        meaning: "The app asked for more of a resource — memory, most often — than its \
                  manifest allows. The message names the limit and the amount allowed.",
        recovery: "Raise the matching limit under `capabilities.limits` in `clwd.toml` and \
                   rebuild, or change the app to use less.",
        action: Recovery::FixInput,
    },
    Row {
        code: "runtime.trap",
        meaning: "The app itself failed — it panicked, hit an assertion, or executed \
                  something invalid. This is a bug in the app being run, not in clwd.",
        recovery: "The message carries the app's own failure text and a backtrace. Fix the \
                   app, rebuild with `clwd build`, and run it again.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "runtime.capability_denied",
        meaning: "The app tried to use something its manifest does not grant: an outbound \
                  request to a host not in `capabilities.egress`, or persistent storage \
                  while `capabilities.storage_namespaces` is empty. Permissions are \
                  deny-by-default — anything not granted is refused — so this usually means \
                  a grant is missing rather than that anything is broken.",
        recovery: "Add the host to `egress`, or a name to `storage_namespaces` to enable \
                   app-data, in `clwd.toml` under `[capabilities]`, then rebuild with `clwd \
                   build`. If the app should not be reaching that host at all, the denial \
                   is working as intended.",
        action: Recovery::FixInput,
    },
    Row {
        code: "runtime.internal",
        meaning: "The sandbox hit a problem it does not have a specific explanation for. \
                  This is a defect in clwd rather than in the app.",
        recovery: "Nothing the caller can change will help. Report it at \
                   https://github.com/clwdrun/clwd/issues.",
        action: Recovery::ReportBug,
    },
    // ---- bundle.* — decided by clwd-bundle ----
    //
    // The bulk of the table, and the most technical. A bundle is the single file
    // `clwd build` produces: an archive holding the app plus a `clwd.toml`
    // describing it. Almost every code here means that file is damaged or was
    // edited after it was built, and almost every recovery is "build it again" —
    // which is the useful thing to say, rather than explaining tar.
    Row {
        code: "bundle.not_tar",
        meaning: "The file is not an app bundle at all. clwd could not read it as an \
                  archive.",
        recovery: "Check the path points at a file `clwd build` produced. If it does, the \
                   file is damaged — build it again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.magic_invalid",
        meaning: "The bundle's internal structure is corrupt. Part of it that identifies \
                  the archive format is wrong.",
        recovery: "The file was damaged in transit or storage. Build it again with `clwd \
                   build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.entry_type_invalid",
        meaning: "The bundle contains something other than an ordinary file — a symbolic \
                  link, a directory entry, or a device node. Bundles may only hold plain \
                  files, so that unpacking one can never write outside its own folder.",
        recovery: "Remove the link or special file from your project and run `clwd build` \
                   again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.prefix_not_empty",
        meaning: "The bundle uses an older archive layout that clwd does not accept, \
                  because the same file can be described two ways in it and two \
                  descriptions of one file mean two different content addresses.",
        recovery: "The bundle was not produced by this version of clwd. Build it again \
                   with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.mode_invalid",
        meaning: "A file in the bundle has unexpected permissions. Bundles allow only \
                  ordinary read and executable permissions, so that the same project \
                  always produces the same bundle on every machine.",
        recovery: "Reset the file's permissions in your project — `chmod 644` for data, \
                   `chmod 755` for programs — and run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.metadata_not_canonical",
        meaning: "A file in the bundle carries machine-specific details such as a \
                  timestamp or an owner name. Bundles deliberately leave these out, so \
                  that building the same project twice produces an identical file.",
        recovery: "The bundle was not produced by `clwd build`, or was rewritten by \
                   another tool afterwards. Build it again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.checksum_invalid",
        meaning: "A self-check inside the bundle failed. The file has been damaged.",
        recovery: "Build it again with `clwd build`. If it keeps happening, the storage or \
                   network carrying the file is corrupting it.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.size_encoding_invalid",
        meaning: "The bundle records the length of one of its files in a form clwd does \
                  not accept. The file is corrupt or was written by a different tool.",
        recovery: "Build it again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.member_too_large",
        meaning: "One file inside the bundle is larger than the 8 GiB maximum a bundle can \
                  describe.",
        recovery: "Remove or shrink the oversized file — large assets usually belong in \
                   storage the app fetches at run time, not inside the app itself — then \
                   run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.padding_invalid",
        meaning: "Unused space inside the bundle contains stray data rather than being \
                  empty. The file has been damaged or modified.",
        recovery: "Build it again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.entries_not_sorted",
        meaning: "The files inside the bundle are not in the fixed order clwd requires. \
                  The order is part of what makes the same project always produce the same \
                  bundle.",
        recovery: "The bundle was assembled or rewritten by something other than `clwd \
                   build`. Build it again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.trailer_invalid",
        meaning: "The bundle is cut short or has extra data after its end. It was \
                  truncated in transit, or two files were joined together.",
        recovery: "Download or build the bundle again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.path_invalid",
        meaning: "A file inside the bundle has a name clwd will not accept — an absolute \
                  path, one containing `..`, or one with characters that are unsafe to \
                  write to disk. This is what stops a bundle writing outside its own \
                  folder when it is unpacked.",
        recovery: "Rename the file in your project so its path is relative and contains no \
                   `..` segments, then run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.path_not_nfc",
        meaning: "A file name uses accented or non-English characters written in a form \
                  that some systems store differently. macOS and Linux disagree about how \
                  to spell such names, and the disagreement would give the same project \
                  two different content addresses.",
        recovery: "Rename the file, retyping the accented characters rather than pasting \
                   them, then run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.path_too_long",
        meaning: "A file's path inside the bundle is longer than the 100-byte maximum.",
        recovery: "Shorten the folder or file names in your project so the whole path fits \
                   within 100 bytes, then run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.path_duplicate",
        meaning: "The same file path appears twice in the bundle, so which one the app \
                  would actually get is ambiguous.",
        recovery: "The bundle was assembled by something other than `clwd build`. Build it \
                   again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.manifest_missing",
        meaning: "The bundle has no `clwd.toml`. That file is what says which program to \
                  run and what the app is allowed to do, so without it the bundle cannot \
                  be used.",
        recovery: "Add a `clwd.toml` to your project — `clwd init` creates one — then run \
                   `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.manifest_not_toml",
        meaning: "The `clwd.toml` file could not be read. Its text is not valid TOML — \
                  usually a missing quote, bracket, or equals sign.",
        recovery: "The message names the line. Fix the syntax in `clwd.toml` and run `clwd \
                   build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.schema_version_missing",
        meaning: "`clwd.toml` does not say which version of the format it is written in, \
                  so clwd cannot know how to read the rest of it.",
        recovery: "Add `schema_version = 1` at the top of `clwd.toml` and run `clwd build` \
                   again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.schema_version_unsupported",
        meaning: "`clwd.toml` is written in a newer format version than this copy of clwd \
                  understands.",
        recovery: "Update clwd to a version that reads this format, or set `schema_version \
                   = 1` and adjust the file to match the older format.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.unknown_key",
        meaning: "`clwd.toml` contains a setting clwd does not recognise. Unknown settings \
                  are refused rather than ignored, because silently ignoring one would \
                  mean an app runs with permissions its author thought they had granted.",
        recovery: "The message names the key. Remove it or correct the spelling, then run \
                   `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.capability_unknown_key",
        meaning: "`clwd.toml` asks for a permission this version of clwd does not know how \
                  to enforce. Refusing is deliberate: running anyway would give the app \
                  less protection than its author asked for.",
        recovery: "Update clwd to a version that supports the permission, or remove it \
                   from the `[capabilities]` section and run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.capabilities_version_unsupported",
        meaning: "The permissions section of `clwd.toml` is written in a newer format \
                  version than this copy of clwd enforces.",
        recovery: "Update clwd, or set `capabilities.version = 1` and adjust that section \
                   to match, then run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.field_missing",
        meaning: "A setting that `clwd.toml` must contain is absent.",
        recovery: "The message names the missing key. Add it to `clwd.toml` and run `clwd \
                   build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.field_type_invalid",
        meaning: "A setting in `clwd.toml` is the wrong kind of value — a number where \
                  text was expected, or a single value where a list was.",
        recovery: "The message names the key and the kind of value it needs. Correct it in \
                   `clwd.toml` and run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.field_value_invalid",
        meaning: "A setting in `clwd.toml` is the right kind of value but not an allowed \
                  one — a negative limit, or an empty name.",
        recovery: "The message names the key and why the value was refused. Correct it in \
                   `clwd.toml` and run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.runtime_unknown",
        meaning: "`clwd.toml` asks to run the app on a sandbox clwd does not have. The \
                  only choices are `wasm` and `microvm`.",
        recovery: "Set `runtime` to either `wasm` or `microvm` in `clwd.toml` and run \
                   `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.manifest_not_canonical",
        meaning: "`clwd.toml` inside the bundle is not stored in the exact layout clwd \
                  requires. The layout is fixed so that the same project always produces \
                  the same bundle, byte for byte.",
        recovery: "The file was edited after the bundle was built. Build it again with \
                   `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.manifest_self_listed",
        meaning: "`clwd.toml` lists itself among the app's files. It is always present and \
                  is never one of the listed members, so listing it makes the bundle \
                  describe itself in two ways.",
        recovery: "The bundle was assembled by something other than `clwd build`. Build it \
                   again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.member_missing",
        meaning: "`clwd.toml` lists a file as part of the app, but that file is not \
                  actually in the bundle.",
        recovery: "The bundle is incomplete — a build was interrupted, or the file was \
                   removed afterwards. Build it again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.member_unlisted",
        meaning: "The bundle contains a file that `clwd.toml` does not list. Every file \
                  must be listed, so that what runs is exactly what the manifest \
                  describes and nothing was slipped in.",
        recovery: "The bundle was modified after it was built. Build it again with `clwd \
                   build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.member_size_mismatch",
        meaning: "A file inside the bundle is a different size from what `clwd.toml` says \
                  it should be.",
        recovery: "The bundle is damaged or was modified after it was built. Build it \
                   again with `clwd build`.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.member_digest_mismatch",
        meaning: "A file inside the app bundle does not match what the bundle says it \
                  should contain. The bundle is damaged, or was modified after it was \
                  built.",
        recovery: "Build it again with `clwd build`. If the bundle came from somewhere \
                   else, do not run it — the contents are not what its author signed.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.digest_malformed",
        meaning: "A content address in `clwd.toml` is not written in the required form: \
                  `blake3:` followed by 64 lowercase hexadecimal characters.",
        recovery: "The file was hand-edited. Build the bundle again with `clwd build` so \
                   the addresses are written for you.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.entrypoint_not_a_member",
        meaning: "`clwd.toml` names a program to run that is not one of the files listed \
                  in the bundle.",
        recovery: "Correct `app.entrypoint` in `clwd.toml` so it names a file the bundle \
                   actually contains, then run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.wasm_not_component",
        meaning: "The app was built as an older, simpler kind of WebAssembly program than \
                  the `wasm` sandbox can run. It needs to be a component, which is the \
                  form that can declare what it needs from the outside world.",
        recovery: "Build for the `wasm32-wasip2` target — `cargo build --target \
                   wasm32-wasip2` — then run `clwd build` again.",
        action: Recovery::Rebuild,
    },
    Row {
        code: "bundle.wasm_preamble_invalid",
        meaning: "The file `clwd.toml` names as the program is not WebAssembly at all.",
        recovery: "Check that `app.entrypoint` points at the compiled `.wasm` file rather \
                   than at source code, then run `clwd build` again.",
        action: Recovery::FixInput,
    },
    Row {
        code: "bundle.digest_mismatch",
        meaning: "The bundle as a whole does not match the content address it was asked \
                  for. The file that arrived is not the one that was requested.",
        recovery: "The download was corrupted or the wrong file was fetched. Fetch or \
                   build it again. If it keeps happening with a bundle from someone else, \
                   do not run it.",
        action: Recovery::Rebuild,
    },
];

fn docs_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docs/error-codes.md")
}

/// Render the table as the committed markdown page.
fn render() -> String {
    let mut out = String::new();
    out.push_str(
        "<!-- Generated by `just error-table`. Do not edit by hand. -->\n\
         <!-- Source: crates/cli/tests/error_table.rs -->\n\n\
         # clwd error codes\n\n\
         Every failure clwd reports carries a stable code. With `--json`, that code arrives\n\
         in an envelope:\n\n\
         ```json\n\
         {\"code\":\"cli.not_found\",\"message\":\"…\",\"hint\":\"…\",\"retryable\":false}\n\
         ```\n\n\
         `hint` is always present and may be `null`. One JSON object per line, never a\n\
         wrapping array — a `clwd logs --follow` that never ends has no closing bracket to\n\
         write.\n\n\
         ## Reading this table\n\n\
         **The prefix names the subsystem that made the decision**, and that is a guarantee\n\
         rather than a convention. A `bundle.*` code always means the app bundle was\n\
         rejected, whichever command surfaced it.\n\n\
         **Recovery** is a closed set, for callers that branch on a value rather than read\n\
         prose: `retry`, `fix_input`, `fix_permissions`, `rebuild`, `report_bug`. It is\n\
         documentation only — it is deliberately *not* a field in the envelope, because\n\
         adding one is a compatibility change the platform would have to mirror.\n\n\
         **Reachable** says whether a CLI user can hit the code today, or which issue makes\n\
         it reachable. Codes that are not yet reachable are listed rather than omitted, so\n\
         the table is honest about what is coming.\n\n\
         ## Exit codes\n\n\
         | Status | Meaning |\n\
         | --- | --- |\n\
         | `0` | The command succeeded. |\n\
         | `1` | The command ran and failed. An envelope was written to stderr. |\n\
         | `2` | The command line was not accepted; nothing was attempted. |\n\n\
         Codes do not map to distinct exit statuses. With five reachable codes and nothing\n\
         retryable, a per-code mapping would be a second compatibility surface for no gain;\n\
         `retryable` and the recovery column carry that signal instead.\n\n\
         ## All codes\n\n\
         | Code | Recovery | Reachable |\n\
         | --- | --- | --- |\n",
    );

    // An index, because 49 entries is a page someone scrolls rather than reads.
    // A reader who arrives holding a code should find it without paging past
    // thirty unrelated ones, and an agent should be able to take the whole
    // mapping without walking the prose.
    for row in TABLE {
        out.push_str(&format!(
            "| [`{}`](#{}) | `{}` | {} |\n",
            row.code,
            row.code.replace('.', ""),
            row.action.as_str(),
            match Reach::of(row.code) {
                Reach::Now => "yes".to_owned(),
                Reach::Soon(issue) => issue.to_owned(),
            },
        ));
    }
    out.push('\n');

    for (prefix, title, blurb) in [
        (
            "cli.",
            "`cli.*` — the command line itself",
            "Decided by clwd on your machine, before anything is built or run.",
        ),
        (
            "net.",
            "`net.*` — reaching a platform",
            "Decided by clwd on your machine when a command that talks to a platform \
             could not reach one. The request never arrived, so no platform could say \
             what went wrong.",
        ),
        (
            "runtime.",
            "`runtime.*` — running an app",
            "Decided by the sandbox while an app is starting or running.",
        ),
        (
            "bundle.",
            "`bundle.*` — reading an app bundle",
            "Decided when a bundle is read. A bundle is the single file `clwd build` \
             produces: an archive holding your app plus the `clwd.toml` that describes \
             it. Most of these mean that file is damaged or was changed after it was \
             built.",
        ),
    ] {
        // The reachability sentence is generated, never written. A hand-written
        // one goes stale the moment the issue it names lands — two of these said
        // "these reach the CLI once `clwd run` lands (#25)" and were false the
        // instant #25 did. The drift check cannot catch that: it compares the
        // generated page against the committed one, and prose written into the
        // generator matches itself. Deriving the sentence removes the class.
        let reach = match Reach::of(&format!("{prefix}x")) {
            Reach::Now => "Every code here can reach a CLI user today.".to_owned(),
            Reach::Soon(issue) => {
                format!("These are not reachable from the CLI yet; {issue} is what changes that.")
            }
        };
        out.push_str(&format!("## {title}\n\n{blurb} {reach}\n\n"));
        for row in TABLE.iter().filter(|r| r.code.starts_with(prefix)) {
            out.push_str(&format!(
                "### `{}`\n\n{}\n\n**What to do:** {}\n\n*Recovery:* `{}` · *Reachable:* {}\n\n",
                row.code,
                row.meaning,
                row.recovery,
                row.action.as_str(),
                match Reach::of(row.code) {
                    Reach::Now => "yes".to_owned(),
                    Reach::Soon(issue) => format!("not yet — {issue}"),
                },
            ));
        }
    }

    out.push_str(
        "## Changing a code\n\n\
         Codes are a compatibility surface: `--json` emits them, the platform mirrors them,\n\
         and automated callers branch on them. Renaming one is a breaking change.\n\n\
         The generated catalogue and its exhaustiveness test both come from the same macro\n\
         literal, so they move together and cannot catch a rename on their own. This file is\n\
         the external anchor that does not move. To change a code deliberately:\n\n\
         1. Change the literal in the owning crate's `error.rs`.\n\
         2. Update its row in `crates/cli/tests/error_table.rs`.\n\
         3. Run `just error-table` to regenerate this page.\n\
         4. Commit all three together — the diff is the record that it was deliberate.\n\n\
         Skipping step 3 fails `just check`.\n",
    );

    out
}

/// Every code from every catalogue, which is what the table must cover.
fn all_codes() -> BTreeSet<&'static str> {
    clwd_cli::error::CODES
        .iter()
        .chain(clwd_runtime::error::OWNED_CODES)
        .chain(clwd_bundle::error::CODES)
        .copied()
        .collect()
}

/// The recovery vocabulary is published, so its wire names must be distinct and
/// stable. This also exercises every variant, including ones no code carries
/// yet — a category is part of the contract from the moment it is named.
#[test]
fn every_category_has_a_distinct_wire_name() {
    let all = [
        Recovery::Retry,
        Recovery::FixInput,
        Recovery::FixPermissions,
        Recovery::Rebuild,
        Recovery::ReportBug,
    ];
    let names: BTreeSet<&str> = all.iter().map(|r| r.as_str()).collect();
    assert_eq!(names.len(), all.len(), "two categories share a wire name");
    for name in &names {
        assert_eq!(*name, &name.to_lowercase(), "{name} is not lowercase");
        assert!(!name.contains(' '), "{name} has a space; use snake_case");
    }
}

/// `Reach::Soon` is unconstructed today — every namespace the CLI knows about is
/// reachable. It stays because the next namespace to arrive may not be, and its
/// arm is what the generated section sentence uses; deleting it would mean the
/// unreachable case has to be re-invented under time pressure, and the `panic!`
/// in `Reach::of` exists precisely to force that decision.
///
/// This exercises the branch so it is a used variant rather than an `#[allow]`,
/// and pins the sentence it produces.
#[test]
fn the_unreachable_branch_still_renders_a_usable_sentence() {
    let sentence = match Reach::Soon("#99") {
        Reach::Now => unreachable!("constructed as Soon"),
        Reach::Soon(issue) => {
            format!("These are not reachable from the CLI yet; {issue} is what changes that.")
        }
    };
    assert!(sentence.contains("#99"), "the sentence must name the issue");
    assert!(
        sentence.contains("not reachable"),
        "and say plainly that the codes cannot be hit"
    );
}

#[test]
fn table_covers_every_code_and_invents_none() {
    let documented: BTreeSet<&str> = TABLE.iter().map(|r| r.code).collect();
    assert_eq!(
        documented.len(),
        TABLE.len(),
        "a code is documented twice in TABLE"
    );

    let actual = all_codes();
    let undocumented: Vec<_> = actual.difference(&documented).collect();
    let invented: Vec<_> = documented.difference(&actual).collect();

    assert!(
        undocumented.is_empty(),
        "these codes exist but have no row in the table — add one, then run \
         `just error-table`: {undocumented:?}"
    );
    assert!(
        invented.is_empty(),
        "these rows name codes that no crate produces — a code was renamed or removed. \
         If that was deliberate, update the row and run `just error-table`: {invented:?}"
    );
}

#[test]
fn every_row_says_something_useful() {
    for row in TABLE {
        assert!(
            !row.meaning.trim().is_empty(),
            "{} has no meaning",
            row.code
        );
        assert!(
            !row.recovery.trim().is_empty(),
            "{} has no recovery",
            row.code
        );

        // The laziest failure mode: documenting `bundle.member_digest_mismatch`
        // as "member digest mismatch". Crude, but it catches exactly what
        // someone does when adding a variant in a hurry.
        //
        // Only multi-word suffixes are checked. A one-word suffix like `config`
        // or `internal` is an ordinary English word that belongs in good prose —
        // flagging it would push the writing away from the plainest available
        // wording, which is the opposite of the point. Two or more words
        // appearing verbatim and in order is the signal worth having.
        let suffix = row.code.split_once('.').expect("codes are namespaced").1;
        if suffix.contains('_') {
            let restatement = suffix.replace('_', " ");
            let recovery = row.recovery.to_lowercase();
            assert!(
                !recovery.contains(&restatement),
                "{}: the recovery restates the code name ({restatement:?}) instead of \
                 saying what to do",
                row.code
            );
        }

        // A recovery is an action. Every one should tell the reader to do
        // something, and most name a command.
        assert!(
            row.recovery.len() > 30,
            "{}: recovery is too short to be an action: {:?}",
            row.code,
            row.recovery
        );
    }
}

/// Prose in the generated page must not forward-reference an issue as unfinished
/// when the thing it gates is already reachable.
///
/// The reachability *column* is derived, but section blurbs and per-row text are
/// hand-written, and two of them said "these reach the CLI once `clwd run` lands
/// (#25)" — false the moment #25 landed. The drift check cannot see it: it
/// compares the generated page against the committed one, and prose written into
/// the generator matches itself.
///
/// This is a backstop, not a proof. It catches the specific phrasing that goes
/// stale — an issue number next to a word implying it has not happened — for
/// codes now marked reachable. Prose can still be wrong in ways no diff detects;
/// that limit is stated on the page itself.
#[test]
fn reachable_codes_do_not_claim_to_be_waiting_on_an_issue() {
    let page = render();
    let stale: Vec<&str> = page
        .lines()
        .filter(|line| {
            let l = line.to_lowercase();
            l.contains('#')
                && (l.contains("not yet")
                    || l.contains("once ")
                    || l.contains("lands")
                    || l.contains("will reach"))
        })
        .collect();

    assert!(
        stale.is_empty(),
        "these lines gate something on an issue that has already landed, or read as \
         though it has not — regenerate the sentence from `Reach::of` rather than \
         writing it: {stale:#?}"
    );
}

#[test]
fn every_code_is_namespaced_and_lowercase() {
    for code in all_codes() {
        let (ns, rest) = code
            .split_once('.')
            .unwrap_or_else(|| panic!("{code} is not namespaced"));
        assert!(
            !ns.is_empty() && !rest.is_empty(),
            "{code} has an empty half"
        );
        assert_eq!(code, code.to_lowercase(), "{code} is not lowercase");
    }
}

/// The compatibility test.
///
/// `CODES` and the exhaustiveness tests beside it regenerate from the same macro
/// literal, so a rename moves both and neither notices. The committed markdown
/// is the anchor that does not regenerate on its own.
///
/// Set `UPDATE_ERROR_TABLE=1` to rewrite it — that is what `just error-table`
/// does.
#[test]
fn docs_match_the_code() {
    let rendered = render();
    let path = docs_path();

    if std::env::var_os("UPDATE_ERROR_TABLE").is_some() {
        std::fs::write(&path, &rendered).expect("writing docs/error-codes.md");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}. Run `just error-table` to create it.",
            path.display()
        )
    });

    assert_eq!(
        committed, rendered,
        "docs/error-codes.md is stale. If a code changed deliberately, run \
         `just error-table` and commit the result."
    );
}
