# Threat model

What clwd protects, from whom, and how, for the code in this repository: the
runtime, the bundle format, the CLI, the MCP server, the SDK and templates,
and the local dev server.

Every mitigation here cites an invariant in [invariants.md](invariants.md),
and every invariant names the test, recipe or CI step that enforces it.
Anything that is not enforced is under [Known limitations](#known-limitations)
or [Not yet](#not-yet), never under a mitigation. If you find a statement here
that the code does not back, that is a bug in this document; see
[SECURITY.md](../../SECURITY.md).

## What is in scope

| Component | Crate | What it does |
|---|---|---|
| Runtime | `clwd-runtime` | Runs an app's WebAssembly component per request, behind the `Sandbox` trait. The `wasm` backend is the only one implemented. |
| Bundle format | `clwd-bundle` | Encodes, digests and verifies the `.clwd` archive an app ships as ([spec](../spec/bundle-v1.md)). |
| CLI | `clwd-cli` | Builds, runs, deploys and reads the logs of apps; holds the platform token and local secrets. |
| MCP server | `clwd mcp serve` | Exposes six CLI commands as MCP tools over stdio ([docs](../mcp.md)). |
| SDK and templates | `clwd-sdk`, `templates/` | What app code links against, and the starting points `clwd init` copies. |
| Local dev | `clwd dev`, `clwd run` | Serves or runs an app on the developer's machine with a mock caller. |

**The platform.** When an app is deployed, it runs on the hosted service. This
repository sees it only through a contract: the caller's identity reaches the
app through `clwd_sdk::caller()` (the `clwd:identity` interface), and apps
receive no platform cookies or credentials. The hosted service's own threat
model is maintained separately and is out of scope here.

## Assets

1. **The host running guests:** its memory, CPU, files, network and process
   state.
2. **Other apps' state:** their app data, secrets and logs.
3. **Caller identity:** who an app believes is calling it.
4. **The platform token:** the credential the CLI uses to deploy.
5. **The developer's machine:** files, environment and credentials, while
   building, running and driving clwd.
6. **Bundle integrity:** that what runs is exactly what was built and
   reviewed.
7. **The supply chain:** the dependencies and tools that build all of the
   above.

## Actors

| Actor | Wants | Trusted with |
|---|---|---|
| **App author** | To build and ship an app | Their own app and machine. May be a non-engineer, or an agent acting for one. |
| **App visitor** | To use an app | Nothing. Their identity is asserted by the platform, never by them. |
| **Malicious app** | To escape its sandbox, read other apps' data, impersonate a caller, or exhaust the host | Nothing. **The core assumption: every guest component is hostile.** |
| **Malicious bundle** | To make a decoder or the runtime misbehave on crafted bytes | Nothing. |
| **Compromised dependency** | To run code in the build, the CLI, or the runtime | Nothing beyond what its crate's position grants it. |
| **Local attacker** | Another account on the developer's machine, after the token or secrets | Nothing. |
| **Agent driving the MCP server** | To act through the six tools | What the tools expose, inside the workspace root. |
| **Injected text** | App logs and build output written to steer whoever reads them, a terminal or a model | Nothing. |

## Trust boundaries and threats

### 1. Guest ↔ runtime host

The guest is untrusted code compiled to a WebAssembly component and run by
wasmtime inside the host process.

| Threat | Mitigation |
|---|---|
| A guest reads files, environment, arguments or the network without a grant | No ambient authority (RT-15) |
| A guest reaches hosts it was not granted | Egress deny-by-default by hostname (RT-10) |
| A guest reads another app's data or secrets | Data partitioned by the host-named app (RT-12); storage off unless granted (RT-11); secrets only where manifest and store agree (RT-13) |
| A guest impersonates a caller, or sees a previous caller's state | Identity fixed by the host at spawn, no constructor in the guest interface (RT-2, RT-3); a fresh instance per request (RT-1) |
| A guest grants itself authorization | `authorize` fails closed (RT-4) |
| A guest monopolizes CPU, or starves the preemption mechanism | Epoch budget on a dedicated thread (RT-5) |
| A guest exhausts host memory | Linear-memory cap (RT-6), table cap (RT-7), response cap (RT-8), log cap (RT-9) |
| A guest crash takes down other callers | Fault isolation (RT-14) |
| A wasmtime feature widens what a guest can reach | Only the reviewed feature set is built (RT-16) |
| A guest learns why it was denied, to probe the policy | A denied request is a generic error that names no host (RT-10); an ungranted secret looks absent (RT-13) |
| Secrets outlive the instance | `stop` releases the instance and its secrets (RT-18) |

### 2. Bundle bytes ↔ decoder and runtime

| Threat | Mitigation |
|---|---|
| A tampered bundle runs | The digest covers every byte and is checked before parsing (BUN-2); the runtime verifies on every spawn (RT-17) |
| A member path escapes, aliases or differs by platform | Path rules (BUN-3); canonical encoding (BUN-1) |
| A link or special file in the archive | Regular files only (BUN-4) |
| Hidden or mismatched members | Members are exactly what the manifest lists (BUN-5) |
| A manifest key silently ignored, such as a capability misspelled into a grant | Strict manifest (BUN-6) |
| Bytes that are not a component reach the compiler | Components only (BUN-7, RT-17) |

### 3. CLI ↔ platform API

| Threat | Mitigation |
|---|---|
| The token leaks through output, errors or logs | Never rendered (CLI-2) |
| Another local account reads the token | Environment or owner-only file (CLI-1) |
| A deploy claims the wrong host label | Label only from `clwd.toml`, checked before building; MCP requires confirmation (CLI-8) |
| Two deploys of one project interleave | Per-project lock (CLI-7) |
| The token is sent over plain HTTP | **Not mitigated**: see [#102](https://github.com/clwdrun/clwd/issues/102) under Known limitations |

### 4. CLI ↔ cargo and build scripts

`clwd build` and `deploy` run cargo, which runs the project's build scripts
and proc macros: arbitrary code.

| Threat | Mitigation |
|---|---|
| A build script reads the platform token from the environment | Every `CLWD_*` variable is removed from every subprocess (CLI-3) |
| A build script reads or corrupts the MCP protocol stream | Every subprocess has stdin closed (CLI-3); stdout is protocol only (MCP-1) |
| A build script does anything else the user could do | **Not mitigated**: build code runs with the user's permissions (Known limitations) |

### 5. MCP client (an agent) ↔ server

| Threat | Mitigation |
|---|---|
| The agent acts outside the intended directory | Workspace root (MCP-3) |
| The agent irreversibly claims a host label by mistake | `deploy` is annotated destructive and needs `confirm_host_label` (MCP-4, CLI-8) |
| The agent reads or sets secrets | No secrets tool (MCP-4) |
| A cancelled build leaves a half-written bundle | Nothing is left behind (MCP-5) |
| Server output corrupts the protocol | stdout carries protocol messages only (MCP-1); a test-only tool never ships (MCP-6) |

### 6. App output ↔ terminal or agent

| Threat | Mitigation |
|---|---|
| Log text drives the terminal (escape sequences, clipboard writes, reordering marks) | Rendered inert (CLI-4) |
| Log text is read by a model as instructions | Returned only as structured data, escaped (MCP-2) |
| Output volume floods the reader | Capped per invocation, with drops recorded (RT-9) |

### 7. Dev server ↔ localhost

| Threat | Mitigation |
|---|---|
| Another machine reaches the dev server and is treated as the mock caller | Loopback only (DEV-1) |
| The mock caller reaches production | It is a spawn argument, not part of the bundle, and visibly local (DEV-2, BUN-6) |
| The CLI waits on a prompt nobody can answer, or reads the protocol as input | No prompts (CLI-10) |

### 8. App code built from the SDK and templates

These protect an app's visitors from the app's own mistakes in the code paths
clwd provides; they do not constrain what an author writes.

| Threat | Mitigation |
|---|---|
| A browser sniffs a page as something else | `Response::html` sets the type and `nosniff` (SDK-1) |
| Caller-supplied text becomes markup | Escaping (SDK-2) |
| One caller reads another's per-caller data | Keys are scoped by subject (SDK-3) |
| Error messages reflect caller input | They carry none (SDK-4) |
| A template teaches apps to do their own auth, or trust request headers | No auth code; identity only from `caller()` (TPL-1) |
| A template ships with network or secret grants | None granted (TPL-3) |

### 9. Upstream ↔ the build

| Threat | Mitigation |
|---|---|
| A vulnerable or unlicensed dependency | `cargo deny` on every change (SC-1); daily advisories that open an issue (SC-2) |
| A committed credential | gitleaks on full history and pre-commit (SC-3) |
| A tampered CI tool | Pinned and checksum-verified (SC-4) |
| A release from an unreviewed commit | Tags only from green `main` (SC-5); Releases only by hand (SC-6) |
| The bundle spec or its fixtures drift | Sealed by checksum (SC-7) |

## Sandbox-escape classes

Known ways out of a WebAssembly sandbox, and where each stands.

| Class | Status | Mitigation | Tracking |
|---|---|---|---|
| A bug in wasmtime's compiler or runtime (miscompiled bounds check, type confusion) | Reduced, not eliminated | A pinned wasmtime with only reviewed features (RT-16); advisories on every change and daily (SC-1, SC-2). A second isolation layer does not exist yet. | [#21](https://github.com/clwdrun/clwd/issues/21), [#22](https://github.com/clwdrun/clwd/issues/22), [#23](https://github.com/clwdrun/clwd/issues/23) |
| A wasmtime feature that widens the attack surface (for example the async component model behind GHSA-32h6-97mm-8q3c) | Mitigated | Not compiled in (RT-16) | |
| A bug in a host function clwd provides (identity, secrets, app data, outbound HTTP) | Mitigated by test | RT-2 to RT-4, RT-10 to RT-13, and the conformance suite | |
| Ambient WASI authority (files, sockets, environment) | Mitigated | RT-15 | |
| Exhausting the host from inside one guest | Partly mitigated | RT-5 to RT-9; per-host admission and request bodies are not bounded here | [#48](https://github.com/clwdrun/clwd/issues/48), [#104](https://github.com/clwdrun/clwd/issues/104) |
| State or identity leaking between callers | Mitigated | RT-1, RT-2 | |
| Crafted bundle bytes | Mitigated for integrity; memory use is the caller's to bound | BUN-1 to BUN-7, RT-17 | [#74](https://github.com/clwdrun/clwd/issues/74) |
| Side channels (Spectre-class, timing) | Not addressed by clwd | Nothing beyond wasmtime's defaults, which clwd does not test | |

**Instance per caller.** The `wasm` backend gives every request a new
instance built from the caller identity its handle was spawned with (RT-1,
RT-2), so no instance is ever shared between callers or reused across
requests. A backend that keeps a long-lived instance would have to scope
identity per request instead; none exists today.

## Known limitations

Each is true today. Where it should change, an issue tracks it.

1. **The platform URL accepts `http://`,** so a mistyped URL sends the token in
   clear text. [#102](https://github.com/clwdrun/clwd/issues/102) (P1).
2. **Build scripts and proc macros run with the user's permissions.** `build`
   and `deploy` run cargo, which runs project code as ordinary code. The MCP
   workspace root confines where the tools act, not what that code does.
   Inherent to building Rust; not tracked.
3. **cargo reads `.cargo/config.toml` from the project's parent directories,**
   including above the MCP workspace root. Inherent to cargo; not tracked.
4. **The MCP root check has a timing gap:** a symlink swapped in between the
   check and the use is followed. [#105](https://github.com/clwdrun/clwd/issues/105).
5. **Cancelling a build on Windows kills only cargo,** not the compiler
   processes it started. [#106](https://github.com/clwdrun/clwd/issues/106).
6. **A runtime escape lands in the host process,** with whatever that process
   can reach. A deployment that runs more than one tenant in one process must
   isolate tenants itself. The planned second layer (microVM, jailer, seccomp,
   landlock) is not built. [#21](https://github.com/clwdrun/clwd/issues/21), [#22](https://github.com/clwdrun/clwd/issues/22), [#23](https://github.com/clwdrun/clwd/issues/23).
7. **`cpu_ms` is charged in wall time while an invocation is active,**
   including time spent in host calls, and checked at the next point inside
   guest code. It errs toward stopping a guest early. [#107](https://github.com/clwdrun/clwd/issues/107).
8. **Limits are per instance, not per host.** Many guests each within budget
   can still exhaust a machine, and host-side allocations (the buffered request
   body, the resource table) are outside the per-instance limits. [#48](https://github.com/clwdrun/clwd/issues/48).
9. **The runtime does not cap the request body;** the embedding host must. [#104](https://github.com/clwdrun/clwd/issues/104).
10. **Egress is checked by hostname, not address.** DNS rebinding and allowed
    names that resolve to internal addresses are for the network edge to stop.
    Under `clwd dev` and `clwd run` there is no edge, so an allowed name that
    resolves to a local address is reachable. That runs the author's own code
    on the author's machine; not tracked.
11. **`storage_namespaces` is on or off;** the namespace strings grant nothing
    narrower than the whole store. [#108](https://github.com/clwdrun/clwd/issues/108).
12. **The local secret store is plaintext,** protected by file permissions
    (CLI-5) against other accounts, not against the account that owns it. By
    design for a development machine; not tracked.
13. **The runtime is tested only on x86-64 Linux.** Nothing prevents building
    it elsewhere; nothing tests it there. aarch64 is [#47](https://github.com/clwdrun/clwd/issues/47).
14. **Decoding a bundle needs the whole bundle in memory,** and a member may be
    up to 8 GiB, so a caller decoding untrusted bytes must cap their size
    first. [#74](https://github.com/clwdrun/clwd/issues/74).
15. **Advisories are checked on every change and daily, not continuously.** A
    new advisory against an unchanged lockfile can go unseen for up to a day.
    Not tracked.
16. **GitHub's own `actions/*` are pinned by tag,** not commit SHA, so the code
    those steps run can change without a commit here. [#103](https://github.com/clwdrun/clwd/issues/103).
17. **Side channels are not addressed** beyond wasmtime's defaults. Not
    tracked.

## Not yet

Planned, and claimed nowhere above:

- **The `microvm` backend.** `MicroVmSandbox` is a placeholder type; no
  invariant here applies to it. [#21](https://github.com/clwdrun/clwd/issues/21) to [#24](https://github.com/clwdrun/clwd/issues/24), [#35](https://github.com/clwdrun/clwd/issues/35) to [#38](https://github.com/clwdrun/clwd/issues/38).
- **The jailer, seccomp and landlock layering** around the VM, and the
  rationale for it, belong with that backend. [#23](https://github.com/clwdrun/clwd/issues/23).
- **A per-namespace storage capability.** [#108](https://github.com/clwdrun/clwd/issues/108).
- **Per-sandbox concurrency limits and admission control.** [#48](https://github.com/clwdrun/clwd/issues/48).

## Out of scope

- **The hosted service.** Its own threat model is maintained separately.
  Reports about it go through the same channel as everything else; see
  [SECURITY.md](../../SECURITY.md).
- **Bugs in apps people deploy,** unless the bug is that clwd failed to
  contain them.
- **Volumetric denial of service** against hosted infrastructure.
- **An attacker who already controls the developer's account.** They have the
  token, the secrets and the source; nothing on the machine protects them from
  that account.
