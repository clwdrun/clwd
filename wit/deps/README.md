# Vendored WIT dependencies

Pinned, not fetched at build time. A component's contract must not change
because an upstream repo moved.

## First-party

`identity/` is **ours**, not vendored. It lives here because a WIT directory
is a single package, and `clwd:identity` is a separate package from the
`clwd:app` world at `wit/world.wit`. Edit it directly; it has no upstream.
Future capability packages (`clwd:secrets`, `clwd:app-data`) land here too.

## Vendored

| Package | Version | Source |
|---|---|---|
| `wasi:http` | 0.2.0 | WebAssembly/wasi-http @ `v0.2.0` |
| `wasi:io` `wasi:clocks` `wasi:random` `wasi:cli` `wasi:filesystem` `wasi:sockets` | 0.2.0 | same tree, `wit/deps/` |

All six transitive packages come from the wasi-http tag rather than their own
repos, so the set is guaranteed self-consistent.

To refresh the vendored set, bump the tag and re-vendor wholesale — do not hand-edit these files:

```
curl -sSL -o /tmp/wasi-http.tar.gz \
  https://github.com/WebAssembly/wasi-http/archive/refs/tags/vX.Y.Z.tar.gz
tar xzf /tmp/wasi-http.tar.gz -C /tmp
cp /tmp/wasi-http-X.Y.Z/wit/*.wit          wit/deps/http/
cp -r /tmp/wasi-http-X.Y.Z/wit/deps/*      wit/deps/
just wit-check
```
