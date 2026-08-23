# clwd

A cloud for small software. Public workspace (Apache-2.0): sandbox runtime,
CLI, SDK, and templates.

## Crates

- `clwd-cli` — binary: `clwd deploy | logs | share | secrets`
- `clwd-sdk` — what app code links against
- `clwd-runtime` — the `Sandbox` trait, with `wasm` and `microvm` backends
  gated behind Cargo features
- `clwd-agent` — musl binary that runs inside the microVM (build with
  `cargo build -p clwd-agent --target x86_64-unknown-linux-musl`)

## Build

```bash
cargo build --workspace          # cli, sdk, runtime (no feature flags)
cargo build -p clwd-runtime --features wasm
cargo build -p clwd-runtime --features microvm
```

## Security

The isolation boundary is the product, and this repo is public so it can be
audited. Report vulnerabilities privately — see [SECURITY.md](SECURITY.md).
Do not open a public issue for a suspected sandbox escape.
