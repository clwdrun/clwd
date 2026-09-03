# clwd workspace tasks

set shell := ["bash", "-uc"]

default:
    @just --list

# ==========================================
# 1. GATE
# ==========================================

# The gate. Everything CI enforces, in the order that fails cheapest first.
check: secrets fmt-check clippy test test-wasm test-conformance lib-silent lib-bare cli-deps spec-intact guest-intact templates-build bundle-deps trait-deps wasm-contained deny

# Secret scan over full history. GitHub's own scanning and push protection are
# Advanced Security features and are not available on a private repository, so
# this replaces them. `detect` walks every commit rather than the working tree:
# a credential already committed is the case that matters, and a clean history
# is a precondition for this repo going public at MVP.
#
# Fails loudly when gitleaks is absent. A check that no-ops when its tool is
# missing is worse than no check, because it reports green.
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

deny:
    cargo deny check --hide-inclusion-graph

# The library half of clwd-cli must stay silent — `clwd mcp serve` calls it
# in-process and emits nothing on stdout but JSON-RPC. main.rs is exempt:
# printing is its job.
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
    wasm-tools component wit wit/

# ==========================================
# 4. CLEANUP
# ==========================================

clean:
    cargo clean

# Rebuild the wasm test guest. Its output is committed so CI needs no guest
# toolchain; run this after changing wit/ or the guest source.
rebuild-guest:
    cd crates/runtime/tests/fixtures/guest && cargo build --release --target wasm32-wasip2
    cp crates/runtime/tests/fixtures/guest/target/wasm32-wasip2/release/clwd_test_guest.wasm \
       crates/runtime/tests/fixtures/app.wasm
