# clwd workspace tasks

set shell := ["bash", "-uc"]

default:
    @just --list

# ==========================================
# 1. GATE
# ==========================================

# The gate. Everything CI enforces, in the order that fails cheapest first.
check: fmt-check clippy test lib-silent lib-bare deny

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-features -- -D warnings

test:
    cargo test --workspace

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
