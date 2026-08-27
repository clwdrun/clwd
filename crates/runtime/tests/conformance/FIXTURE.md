# Conformance fixture contract

**Status: authoritative.** This document defines what a conforming *test guest*
must do. It is versioned the way `docs/spec/bundle-v1.md` is: a change here lands
in its own commit, before any code that implements or depends on the change. A
backend arm of the conformance suite (`crates/runtime/tests/conformance.rs`) is
only meaningful if the guest it runs behaves exactly as written below.

## Why there are two contracts

Conformance is two things, not one:

- **Contract A — backend behaviour.** The `Sandbox` implementation must satisfy
  every requirement in the registry (`clwd_runtime::conformance::REQUIREMENTS`).
  That is the list the suite enumerates and tests one by one.
- **Contract B — guest behaviour.** *This document.* Every requirement in
  contract A is exercised by driving a guest and asserting on what comes back.
  The guest must respond predictably, in terms that do not depend on which
  backend is hosting it. Without a written contract B, issue #21 has a suite it
  cannot run and no definition of the artifact it must build.

The wasm reference guest lives at `crates/runtime/tests/fixtures/guest/` and is
the executable reference implementation of this contract. The microVM guest that
#21 builds — a rootfs plus a `clwd-agent` request handler — must answer the same
request table with the same response bodies, over the same `clwd:*` host
interfaces (via vsock rather than component imports).

The suite's `fixture_self_check` test drives the whole request table against
whichever backend arm is available and asserts this contract, so a new guest
gets a red/green signal against it directly.

## Request table

All requests are `GET`. The path selects the behaviour. Unless stated otherwise
the response is `200` with the body given verbatim (no trailing newline).

| Path | Behaviour | Response body |
| --- | --- | --- |
| `/` | Liveness. Do nothing else. | `ok` |
| `/whoami` | Return the calling subject id. An anonymous caller returns the literal `anonymous`. | subject id, or `anonymous` |
| `/role` | Return the caller's role on this app, or `none` when there is no role. | `viewer` / `editor` / `owner` / `none` |
| `/groups` | Return the caller's group ids, comma-joined, in the order the host reports them. Empty when the caller is in no groups. | e.g. `eng,oncall`, or `` |
| `/log` | Write exactly one line to stdout whose text contains `stdout`, and exactly one line to stderr whose text contains `stderr`. | `logged` |
| `/flood-logs` | Write more than 1 MiB to stdout in a single invocation, as a few hundred multi-KiB lines. Forces the host's log buffer past its capacity. | `flooded` |
| `/trap` | Crash the guest hard: `panic!`, `unreachable`, `abort` — whatever terminates the current invocation abnormally. There is no response. | — (the invocation fails) |
| `/spin` | Busy-loop forever. Do **not** yield, sleep, block on host I/O, or allocate. Only host preemption stops this. | — (the invocation is preempted) |
| `/sleep/{ms}` | Block on a host timer for `{ms}` milliseconds without consuming CPU, then return. Used to separate a wall-clock deadline from a CPU budget. | `slept` |
| `/alloc` | Allocate linear memory without bound, in steps of at least 64 KiB, touching (writing to) at least one byte of every step so the allocation is realised. Do not free. | — (the invocation fails with a resource error) |
| `/exit` | Terminate this instance cleanly from inside the guest — a normal process/component exit, not a crash. The current invocation may or may not return a body; the *next* invocation on the same handle must observe the instance as gone. | — (see note) |
| `/secret/{name}` | Read `{name}` via `clwd:secrets`. Return the value as a UTF-8 string. Return the literal `absent` when the broker returns `none`. | secret value, or `absent` |
| `/data/get/{key}` | `clwd:app-data` get. Return the value as a UTF-8 string; `absent` when the key is unset; `error:invalid-key:{why}` when the key is rejected. | value / `absent` / `error:invalid-key:…` |
| `/data/set/{key}/{value}` | `clwd:app-data` set. A missing `{value}` segment means the empty value, which is legal. | `stored` / `error:invalid-key:…` / `error:value-too-large:{limit}` |
| `/data/delete/{key}` | `clwd:app-data` delete. Deleting an unset key succeeds. | `deleted` / `error:invalid-key:…` |
| `/data/big/{n}` | `clwd:app-data` set of a value that is `{n}` bytes long, under a fixed key. Used to reach the host's value ceiling. | `stored` / `error:value-too-large:{limit}` |

### Error rendering

Host-interface errors are rendered into the response body so a test can assert
on which one occurred, with this exact shape:

- `error:invalid-key:{why}` — `clwd:app-data` rejected the key. `{why}` is the
  host's reason string, asserted only as a prefix (`error:invalid-key`).
- `error:value-too-large:{limit}` — `clwd:app-data` rejected the value. `{limit}`
  is the host's own byte limit, which every host sets to at least `1048576`.
- `error:unavailable` — `clwd:app-data` reported the store unavailable. Not
  expected under a conforming in-memory store; present so the guest never
  panics on it.

## Host-interface expectations

The guest calls three first-party interfaces. Their contracts are in `wit/`; the
points that matter for the fixture:

- **`clwd:identity`** — ambient (`get-caller()` takes no argument). `/whoami`,
  `/role`, `/groups` read it directly. The guest never constructs an identity.
- **`clwd:secrets`** — `broker.get(name)` returns `option<list<u8>>` and cannot
  fail. `none` covers both "no such secret" and "not granted to this app"; the
  guest must render both as `absent` and must not branch on the difference.
- **`clwd:app-data`** — a per-app key-value namespace. The key grammar (1–128
  bytes of `a`–`z`, `0`–`9`, `.`, `_`, `-`, not beginning or ending with a
  separator) is enforced by the host, not the guest; the guest passes the key
  through and renders whatever error comes back.

## Notes on the non-obvious paths

- **`/spin` must not yield.** A guest that yields to the host between iterations
  is testing cooperative scheduling, not preemption. R4.1 and R4.6 require that a
  guest which never cedes control is still stopped.
- **`/alloc` must touch each step.** An allocation that is reserved but never
  written may never grow the guest's memory, so the host's memory cap is never
  reached and R4.4 cannot fire.
- **`/exit` exists so every backend has an honest way to reach
  `runtime.stopped`.** The wasm backend builds a fresh instance per invocation,
  so its next call after `/exit` succeeds — `/exit` there is a normal return, and
  R2.2 is instead reached by the harness dropping the instance. A long-lived
  backend (microVM) genuinely tears the instance down on `/exit`, and its next
  call returns `runtime.stopped`. The requirement is: *after the instance is
  gone, the next invocation reports `runtime.stopped`* — `/exit` plus a
  harness-level teardown is how each arm produces that state without relying on a
  scripted `die_after_invokes` knob.
- **`/flood-logs` size and shape.** 1 MiB comfortably exceeds the wasm sink's
  per-stream per-invocation capacity (256 KiB). Multi-KiB lines rather than tiny
  ones are deliberate: a short-line flood produces thousands of dropped lines,
  and a log subscriber's own bounded buffer then loses the host's drop-marker
  along with them, so R7.2 could not observe it. A backend with a larger buffer
  may need the suite to raise the total; the contract is "enough to force a
  drop", and the suite asserts a `[clwd] … dropped` marker line, not a byte
  count.

## What issue #21 must build

To run this suite against the microVM backend, #21 must supply:

1. A microVM guest artifact — a bundle with `runtime = "microvm"` — whose request
   handler answers every row of the request table with the response body given,
   speaking `clwd:identity` / `clwd:secrets` / `clwd:app-data` to the host over
   the `clwd-agent` vsock channel.
2. A `BackendUnderTest` implementation (`clwd_runtime::conformance`) whose
   `fixture_bundle(FixtureKind::Guest)` returns that artifact, whose
   `fixture_bundle(FixtureKind::IngestFailure | Tampered)` returns a
   backend-appropriate broken bundle, whose `instances()` reports every caller
   each instance observed, and whose `evict()` tears an instance down so a
   later invoke on its handle reports `runtime.stopped`.
3. `available()` returning `true` once the above exist. Until then the microVM
   arm reports every requirement as skipped, visibly, per the suite's skip
   mechanism.
