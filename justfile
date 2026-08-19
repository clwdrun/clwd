# clwd master build orchestrator

set shell := ["bash", "-c"]

# Default command when running `just`
default:
    @just --list

# ==========================================
# 1. DEVELOPMENT & CHECKING
# ==========================================

# Run fast type and compiler checks across all Rust crates
check:
    cargo check --workspace

# Run tests across all workspace crates
test:
    cargo test --workspace

# Format all Rust code
fmt:
    cargo +nightly fmt --all

# ==========================================
# 2. BINDINGS & SDK GENERATION
# ==========================================

# Build all language bindings (Python + TypeScript)
build-bindings: build-py build-ts

# Build Python SDK wheel locally via Maturin
build-py:
    @echo "🐍 Building Python SDK..."
    cd bindings/py && maturin develop

# Build TypeScript / Node.js native addon and .d.ts files via NAPI-RS
build-ts:
    @echo "🟦 Building TypeScript SDK..."
    cd bindings/ts && npm run build

# Generate language types directly from WIT files
generate-wit:
    @echo "⚙️ Generating types from wit/..."
    # Generate TypeScript definitions from WIT using jco
    npx @bytecodealliance/jco types wit/ -o bindings/ts/wit-types

# ==========================================
# 3. RELEASE PACKAGING
# ==========================================

# Package production artifacts for distribution (PyPI wheels & NPM packages)
release-all:
    @echo "📦 Packaging for production..."
    mkdir -p dist
    cd bindings/py && maturin build --release --out ../../dist/wheels
    cd bindings/ts && napi build --platform --release --output-dir ../../dist/npm

# ==========================================
# 4. CLEANUP
# ==========================================

# Clean all target directories, wheels, and node artifacts
clean:
    cargo clean
    rm -rf dist/
    rm -rf bindings/ts/*.node bindings/ts/*.d.ts