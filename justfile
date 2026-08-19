# clwd workspace tasks

set shell := ["bash", "-uc"]

default:
    @just --list

# ==========================================
# 1. GATE
# ==========================================

# The gate. Everything CI enforces, in the order that fails cheapest first.
check: fmt-check clippy test deny

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

# ==========================================
# 3. GUEST BINDINGS
# ==========================================
# wit/ is the single source of truth. Bindings are generated build output —
# they land in target/ and are never committed. Both recipes need external
# toolchains, so neither is wired into `check`.

bindings: bindings-py bindings-ts

# Requires: pip install componentize-py
bindings-py:
    componentize-py --wit-path wit --world app bindings target/bindings/py

# Requires: npx @bytecodealliance/jco
bindings-ts:
    npx --yes @bytecodealliance/jco guest-types wit --world-name app -o target/bindings/ts

# Parse-check the WIT package without generating anything.
# Requires: cargo install wasm-tools
wit-check:
    wasm-tools component wit wit/

# ==========================================
# 4. CLEANUP
# ==========================================

clean:
    cargo clean
