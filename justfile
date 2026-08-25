# clwd workspace tasks

set shell := ["bash", "-uc"]

default:
    @just --list

# ==========================================
# 1. GATE
# ==========================================

# The gate. Everything CI enforces, in the order that fails cheapest first.
check: fmt-check clippy test test-wasm lib-silent lib-bare spec-intact bundle-deps trait-deps wasm-contained deny

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

# docs/spec defines bytes, including whitespace inside code spans, and the
# fixtures are the conformance vector. Catch a formatter reflowing either here
# rather than as an unexplained digest mismatch later.
spec-intact:
    cd docs/spec && sha256sum -c SHA256SUMS

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
    cd crates/runtime/tests/fixtures/guest && rm -rf wit && cp -r ../../../../../wit wit
    cd crates/runtime/tests/fixtures/guest && cargo build --release --target wasm32-wasip2
    cp crates/runtime/tests/fixtures/guest/target/wasm32-wasip2/release/clwd_test_guest.wasm \
       crates/runtime/tests/fixtures/app.wasm
