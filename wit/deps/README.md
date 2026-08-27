# Vendored WIT dependencies

Pinned, not fetched at build time. A component's contract must not change
because an upstream repo moved.

## First-party

`identity/`, `secrets/` and `app-data/` are **ours**, not vendored. They live
here because a WIT directory is a single package, and each is a separate
package from the `clwd:app` world at `wit/world.wit`. Edit them directly; they
have no upstream.

### Versioning first-party packages

These are a published contract. `clwd-sdk` ships against them and the platform
pins this repository by tag, so both sides need the same rule for what a
version means.

**Breaking — requires a new package version:**

- changing an existing function's parameters or results
- adding, removing, renaming or retyping a record field
- adding or removing an `enum` or `variant` case
- removing a function or an interface

**Additive — no version bump:**

- adding a new interface to a package
- adding a new function to an existing interface

**The asymmetry in that last one is the part to remember.** Adding a function
is additive *for guests* — every existing app keeps compiling and running,
because it simply never calls the new one. It is **breaking for hosts**, which
must now implement it or fail to link. Both sides read this file, and
"additive" quietly means two different things depending on which side you are.

A change that is additive for guests and breaking for hosts still needs a
version bump if any host outside this repository implements the interface.
Within this repository the wasm backend is the only host, so such a change can
land in one commit — but the moment the platform implements one of these
directly, that stops being true.

## Vendored

| Package | Version | Source |
| --- | --- | --- |
| `wasi:http` | 0.2.0 | WebAssembly/wasi-http @ `v0.2.0` |
| `wasi:io` `wasi:clocks` `wasi:random` `wasi:cli` `wasi:filesystem` `wasi:sockets` | 0.2.0 | same tree, `wit/deps/` |

All six transitive packages come from the wasi-http tag rather than their own
repos, so the set is guaranteed self-consistent.

To refresh the vendored set, bump the tag and re-vendor wholesale — do not hand-edit these files:

```bash
curl -sSL -o /tmp/wasi-http.tar.gz \
  https://github.com/WebAssembly/wasi-http/archive/refs/tags/vX.Y.Z.tar.gz
tar xzf /tmp/wasi-http.tar.gz -C /tmp
cp /tmp/wasi-http-X.Y.Z/wit/*.wit          wit/deps/http/
cp -r /tmp/wasi-http-X.Y.Z/wit/deps/*      wit/deps/
just wit-check
```
