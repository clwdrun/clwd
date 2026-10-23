# clwd workspace tasks

set shell := ["bash", "-uc"]

default:
    @just --list

# ==========================================
# 1. GATE
# ==========================================

# The gate. Everything CI enforces, in the order that fails cheapest first.
check: secrets fmt-check clippy test test-wasm test-conformance lib-silent lib-bare stdout-owners spawn-confined stdin-readers platform-client no-cross-repo-refs mcp-panic mcp-release-bare cli-deps spec-intact guest-intact templates-build bundle-deps trait-deps wasm-contained wasmtime-features invariants deny

# Secret scan over full history. GitHub's own scanning and push protection are
# Advanced Security features and are not available on a private repository, so
# this replaces them. `detect` walks every commit rather than the working tree:
# a credential already committed is the case that matters, and a clean history
# is a precondition for this repo going public at MVP.
#
# Fails loudly when gitleaks is absent. A check that no-ops when its tool is
# missing is worse than no check, because it reports green.
# Invariant: SC-3.
secrets:
    #!/usr/bin/env bash
    set -uo pipefail
    if ! command -v gitleaks >/dev/null 2>&1; then
      echo "error: gitleaks is not installed — install it or this gate is a lie"
      echo "       https://github.com/gitleaks/gitleaks#installing"
      exit 1
    fi
    gitleaks detect --no-banner --redact --exit-code 1

# Install the pre-commit hook. Catching a credential before the commit is a
# non-event; catching it after a push is a rotation.
hooks:
    git config core.hooksPath .githooks
    @echo "ok: core.hooksPath -> .githooks"

# Regenerate docs/error-codes.md after a deliberate code change. Staleness is
# caught by the `docs_match_the_code` test, which `just test` already runs; this
# is the only way to make the change.
error-table:
    UPDATE_ERROR_TABLE=1 cargo test -p clwd-cli --test error_table
    @echo "ok: docs/error-codes.md regenerated — commit it with the code change"

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-features -- -D warnings

test:
    cargo test --workspace

# The default build never compiles the wasm backend, so `cargo test --workspace`
# does not exercise it. A gate that does not compile the code it gates is worse
# than no gate.
test-wasm:
    cargo test -p clwd-runtime --features wasm

# The backend conformance suite is behind its own non-default `conformance`
# feature and also needs `wasm` for the only arm that exists. Neither
# `cargo test --workspace` nor `test-wasm` reaches it. Runs in CI's `backends`
# job alongside `test-wasm`, not the `gate` job (which builds no backend).
test-conformance:
    cargo test -p clwd-runtime --features conformance,wasm --test conformance

# Invariant: SC-1.
deny:
    cargo deny check --hide-inclusion-graph

# The library half of clwd-cli must stay silent — `clwd mcp serve` calls it
# in-process and emits nothing on stdout but JSON-RPC. main.rs is exempt:
# printing is its job.
# Invariant: MCP-1.
lib-silent:
    #!/usr/bin/env bash
    set -uo pipefail
    if grep -rn --include='*.rs' -E '\b(print!|println!|eprint!|eprintln!)' \
         crates/cli/src/lib.rs crates/cli/src/cli.rs \
         crates/cli/src/commands/ crates/cli/src/error.rs; then
      echo "error: print macro in the clwd-cli library surface — see issue #14"
      exit 1
    fi
    echo "ok: no print macro in the clwd-cli library surface"

# Under `clwd mcp serve` stdout is the protocol. `crates/cli/clippy.toml` bans
# taking it except where an `#[expect]` says which owner is taking it: the CLI's
# renderer and the MCP transport, both in main.rs. A third is a new writer to
# a stream one stray byte breaks, so it fails here rather than slipping in.
# Invariant: MCP-1.
stdout-owners:
    #!/usr/bin/env bash
    set -uo pipefail
    sites=$(grep -rn --include='*.rs' 'clippy::disallowed_methods' crates/cli)
    count=$(printf '%s\n' "$sites" | grep -c . || true)
    if [ "$count" -ne 2 ] || printf '%s\n' "$sites" | grep -qv '^crates/cli/src/main.rs:'; then
      echo "error: expected exactly 2 disallowed_methods expectations, both in crates/cli/src/main.rs; found $count:"
      printf '%s\n' "$sites"
      exit 1
    fi
    echo "ok: stdout has exactly two owners, both in main.rs"

# Every subprocess the CLI starts closes stdin and drops every CLWD_* variable
# (the platform token among them). That holds because there is one
# constructor: `build::process::command`. The tests start the binary with
# `Command::new` too, which is why this is a grep over src/ and not a clippy ban.
# Invariant: CLI-3.
spawn-confined:
    #!/usr/bin/env bash
    set -uo pipefail
    sites=$(grep -rn --include='*.rs' -E '\bCommand::new\(' crates/cli/src)
    if [ "$(printf '%s\n' "$sites" | grep -c .)" -ne 1 ] \
       || ! printf '%s\n' "$sites" | grep -q '^crates/cli/src/build/process.rs:'; then
      echo "error: a subprocess is constructed outside build::process::command:"
      printf '%s\n' "$sites"
      exit 1
    fi
    echo "ok: one subprocess constructor, in build::process"

# Nothing in the CLI asks a question and waits. stdin is read in exactly two
# places, both in main.rs: the MCP transport, whose protocol it is, and
# `secrets set`, which reads the value as data. A third reader would be a
# prompt, or a thief of the protocol stream, so it fails here.
# Invariant: CLI-10.
stdin-readers:
    #!/usr/bin/env bash
    set -uo pipefail
    sites=$(grep -rn --include='*.rs' -E '\bstdin\(\)' crates/cli/src)
    if [ "$(printf '%s\n' "$sites" | grep -c .)" -ne 2 ] \
       || printf '%s\n' "$sites" | grep -qv '^crates/cli/src/main.rs:'; then
      echo "error: expected exactly 2 stdin readers, both in crates/cli/src/main.rs:"
      printf '%s\n' "$sites"
      exit 1
    fi
    echo "ok: stdin has exactly two readers, both in main.rs"

# An issue reference into another repository (`repo#N`, `owner/repo#N`) ties
# this public tree to numbers it does not control. State the reason instead.
# References to this repository's own issues (`#N`, `clwdrun/clwd#N`) are fine.
no-cross-repo-refs:
    #!/usr/bin/env bash
    set -uo pipefail
    refs=$(git grep -nIoE '[A-Za-z][A-Za-z0-9_.-]*(/[A-Za-z0-9_.-]+)?#[0-9]+' | grep -vE ':clwdrun/clwd#[0-9]+$')
    if [ -n "$refs" ]; then
      echo "error: an issue reference into another repository:"
      printf '%s\n' "$refs"
      exit 1
    fi
    echo "ok: no issue reference into another repository"

# Every request to the platform carries the token, so every HTTP client in the
# CLI is built by `PlatformUrl::client`, which follows no redirect and, for a
# remote platform, sends nothing over plain http. A client built anywhere else
# would have neither property.
# Invariant: CLI-11.
platform-client:
    #!/usr/bin/env bash
    set -uo pipefail
    sites=$(grep -rn --include='*.rs' -E 'Client::(new|builder)\(|ClientBuilder::new\(' crates/cli/src)
    if [ "$(printf '%s\n' "$sites" | grep -c .)" -ne 1 ] \
       || ! printf '%s\n' "$sites" | grep -q '^crates/cli/src/platform/mod.rs:'; then
      echo "error: an HTTP client is built outside PlatformUrl::client:"
      printf '%s\n' "$sites"
      exit 1
    fi
    echo "ok: one HTTP client constructor, PlatformUrl::client"

# The panic test needs the test-only tool, so it is its own target with
# `mcp-test-panic` as a required feature; `cargo test --workspace` does not
# build it, and this does.
mcp-panic:
    cargo test -p clwd-cli --features mcp-test-panic --test mcp_panic

# The panicking tool must never ship. Checked in the release binary itself, with
# a positive control: a real tool's name is found, so the grep can see names.
# Invariant: MCP-6.
mcp-release-bare:
    #!/usr/bin/env bash
    set -uo pipefail
    cargo build -p clwd-cli --release --bin clwd
    if ! grep -qa 'list_templates' target/release/clwd; then
      echo "error: the release binary has no list_templates — the check below would prove nothing"
      exit 1
    fi
    if grep -qa 'clwd_test_panic' target/release/clwd; then
      echo "error: the release binary contains the test-only panicking tool"
      exit 1
    fi
    echo "ok: the release binary has no test-only tool"

# Terminal-drawing deps are a feature. This asserts the library does not need them.
lib-bare:
    cargo build -p clwd-cli --lib --no-default-features

# A runtime backend reaches the CLI only through the `run` feature. Two
# assertions: absent without it, present with it. The second exists so the gate
# cannot pass because the feature quietly stopped doing anything.
cli-deps:
    #!/usr/bin/env bash
    set -uo pipefail
    if cargo tree -p clwd-cli -e normal --no-default-features | grep -q wasmtime; then
      echo "error: wasmtime reaches clwd-cli without the run feature"
      exit 1
    fi
    if ! cargo tree -p clwd-cli -e normal --features run | grep -q wasmtime; then
      echo "error: the run feature no longer enables a runtime — the check above is meaningless"
      exit 1
    fi
    echo "ok: a runtime reaches clwd-cli only through the run feature"

# docs/spec defines bytes, including whitespace inside code spans, and the
# fixtures are the conformance vector. Catch a formatter reflowing either here
# rather than as an unexplained digest mismatch later.
# Invariant: SC-7.
spec-intact:
    cd docs/spec && sha256sum -c SHA256SUMS

# The guest fixture is `exclude`d from the workspace, so `cargo check
# --workspace` never compiles it, and `test-conformance` runs against the
# committed `app.wasm` rather than a fresh build. Both stay green while the
# fixture's source is broken — which happened during #34, and is why this
# exists. It matters more since then: the guest depends on `clwd-sdk` now, so
# any SDK change can break it.
#
# Cheap on a warm target directory; the first run pays for the wasm build.
# It checks that the fixture COMPILES, not that the committed `app.wasm` matches
# a fresh build. A byte comparison was tried and removed: rustc embeds source
# paths in panic payloads, so the same source produces different bytes from
# different absolute paths — measured at 87,060 vs 87,507, still differing after
# `wasm-tools strip`. Reproducibility would need `--remap-path-prefix` wired
# through here and `rebuild-guest`, which is a build-config change rather than a
# check.
# Templates are shipped product and are excluded from the workspace, so nothing
# else compiles them. Shared target directory: the first costs ~35 s cold and
# each further one ~1.5 s, since clwd-sdk and wit-bindgen compile once.
templates-build:
    #!/usr/bin/env bash
    set -euo pipefail
    for t in templates/*/; do
      (cd "$t" && cargo build --release --target wasm32-wasip2 --target-dir "$PWD/../../target/templates")
    done
    echo "ok: every template builds for wasm32-wasip2"

guest-intact:
    cd crates/runtime/tests/fixtures/guest && cargo build --release --target wasm32-wasip2
    @echo "ok: the guest fixture still compiles against the SDK"

# Re-record the hashes after a deliberate spec or fixture change.
spec-reseal:
    cd docs/spec && sha256sum bundle-v1.md $(ls fixtures/* 2>/dev/null) > SHA256SUMS

# clwd-bundle reads and writes bytes. A runtime dependency here reaches the
# platform builder across the repo seam.
bundle-deps:
    #!/usr/bin/env bash
    set -uo pipefail
    if cargo tree -p clwd-bundle -e normal | grep -qE 'wasmtime|tokio|clwd-runtime'; then
      echo "error: clwd-bundle grew a runtime dependency"
      cargo tree -p clwd-bundle -e normal
      exit 1
    fi
    echo "ok: clwd-bundle has no runtime in its tree"

# The Sandbox trait is the one abstraction the control plane sees, and it must
# not leak a backend type. With no features on, no backend crate may appear.
trait-deps:
    #!/usr/bin/env bash
    set -uo pipefail
    if cargo tree -p clwd-runtime -e normal \
       | grep -qE 'wasmtime|wasi|hyper|firecracker|vsock|landlock|seccompiler'; then
      echo "error: a backend type reached the Sandbox trait surface"
      cargo tree -p clwd-runtime -e normal
      exit 1
    fi
    echo "ok: no backend crate in the trait surface"

# No wasmtime type may appear outside the wasm module, even when the feature is
# on. The trait is what the control plane sees; a wasmtime type in a signature
# elsewhere leaks the backend through it.
wasm-contained:
    #!/usr/bin/env bash
    set -uo pipefail
    escaped=$(grep -rn --include='*.rs' -E '\bwasmtime[_a-z]*::' crates/runtime/src \
              | grep -v '^crates/runtime/src/wasm/' || true)
    if [ -n "$escaped" ]; then
      echo "error: a wasmtime type escaped the wasm module"
      echo "$escaped"
      exit 1
    fi
    echo "ok: wasmtime types confined to the wasm module"

# Every wasmtime feature compiled in is a class of advisory that can reach a
# guest (see the workspace Cargo.toml). This is the exact set, across every
# crate and feature, so adding one fails here and has to be argued for.
# Invariant: RT-16.
wasmtime-features:
    #!/usr/bin/env bash
    set -uo pipefail
    expected='wasmtime feature "addr2line"
    wasmtime feature "async"
    wasmtime feature "backtrace"
    wasmtime feature "component-model"
    wasmtime feature "cranelift"
    wasmtime feature "parallel-compilation"
    wasmtime feature "runtime"
    wasmtime feature "std"
    wasmtime feature "wasmtime-jit-icache-coherence"
    wasmtime feature "wasmtime-unwinder"
    wasmtime-wasi feature "p2"
    wasmtime-wasi-http feature "default-send-request"
    wasmtime-wasi-http feature "p2"'
    expected=$(printf '%s\n' "$expected" | sed 's/^ *//' | sort -u)
    actual=$(for c in wasmtime wasmtime-wasi wasmtime-wasi-http; do
      cargo tree --workspace --all-features -e features -i "$c" | grep -oE "$c feature \"[a-z0-9-]+\""
    done | sort -u)
    if [ "$actual" != "$expected" ]; then
      echo "error: the wasmtime feature set changed (< expected, > actual):"
      diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual")
      exit 1
    fi
    echo "ok: wasmtime builds with exactly the reviewed features"

# docs/security/invariants.md names each invariant once, as `### ID`. Each
# must be enforced somewhere that says so, `Invariant: ID` in a test, a recipe
# here or a workflow step; and every such reference must name an invariant
# that exists. Either direction failing means the document and the code have
# drifted.
invariants:
    #!/usr/bin/env bash
    set -uo pipefail
    doc=docs/security/invariants.md
    ids=$(grep -oE '^### [A-Z]+-[0-9]+' "$doc" | cut -d' ' -f2)
    refs=$(grep -rhoE --exclude-dir=target --exclude-dir=.git 'Invariant: [A-Z]+-[0-9]+' \
             crates templates justfile .github | cut -d' ' -f2 | sort -u)
    if [ -z "$ids" ] || [ -z "$refs" ]; then
      echo "error: found no invariants or no references; this check would prove nothing"
      exit 1
    fi
    fail=0
    dups=$(printf '%s\n' "$ids" | sort | uniq -d)
    [ -n "$dups" ] && { echo "error: invariants defined twice: $dups"; fail=1; }
    unenforced=$(comm -23 <(printf '%s\n' "$ids" | sort -u) <(printf '%s\n' "$refs"))
    [ -n "$unenforced" ] && { echo "error: invariants nothing enforces:" $unenforced; fail=1; }
    unknown=$(comm -13 <(printf '%s\n' "$ids" | sort -u) <(printf '%s\n' "$refs"))
    [ -n "$unknown" ] && { echo "error: references to invariants $doc does not define:" $unknown; fail=1; }
    [ "$fail" -eq 0 ] || exit 1
    echo "ok: $(printf '%s\n' "$ids" | grep -c .) invariants, each enforced, and no unknown reference"

# ==========================================
# 2. BUILDS
# ==========================================

# Default set: cli, sdk, runtime with no backend enabled.
build:
    cargo build --workspace

# Backends are features, never default. Build each explicitly.
build-wasm:
    cargo build -p clwd-runtime --features wasm

build-microvm:
    cargo build -p clwd-runtime --features microvm

# musl-only, deliberately outside default-members.
build-agent:
    cargo build -p clwd-agent --target x86_64-unknown-linux-musl

# Guest components target the component model, not core wasm.
build-sdk:
    cargo build -p clwd-sdk --target wasm32-wasip2

# Parse-check the WIT package without generating anything.
# Requires: cargo install wasm-tools
wit-check:
    wasm-tools component wit crates/sdk/wit/

# ==========================================
# 4. CLEANUP
# ==========================================

clean:
    cargo clean

# Reclaim disk from stale cargo artifacts without a full `cargo clean`. Touches
# only cargo target directories, never sources or wip/. Incremental caches and
# trybuild scratch are regenerated on demand; deps, build-script output and
# fingerprints untouched for `days` are mostly hashes from older lockfiles.
# ponytail: mtime is the staleness signal, so an artifact that is still current
# but unrebuilt for `days` goes too and costs one rebuild; cargo-sweep's
# stamp-based tracking is the upgrade if that ever bites.
clean-stale days="14":
    #!/usr/bin/env bash
    set -euo pipefail
    for t in target crates/runtime/tests/fixtures/guest/target templates/*/target; do
        [ -d "$t" ] || continue
        before=$(du -sh "$t" | cut -f1)
        find "$t" -type d -name incremental -prune -exec rm -rf {} +
        rm -rf "$t/tests/trybuild"
        find "$t" -mindepth 2 \( -path '*/deps/*' -o -path '*/build/*' -o -path '*/.fingerprint/*' \) \
            -prune -mtime +{{days}} -exec rm -rf {} +
        echo "$t: $before -> $(du -sh "$t" | cut -f1)"
    done

# Rebuild the wasm test guest. Its output is committed so CI needs no guest
# toolchain; run this after changing crates/sdk/wit/ or the guest source.
rebuild-guest:
    cd crates/runtime/tests/fixtures/guest && cargo build --release --target wasm32-wasip2
    cp crates/runtime/tests/fixtures/guest/target/wasm32-wasip2/release/clwd_test_guest.wasm \
       crates/runtime/tests/fixtures/app.wasm
