# Development notes — clwd

Architecture, build commands, and the rules that keep the workspace honest.
clwd is a cloud for small software (Apache-2.0): the sandbox runtime, CLI, SDK,
and templates.

## Commands

```bash
cargo build --workspace              # cli, sdk, runtime — no backends enabled
cargo build -p clwd-runtime --features wasm
cargo build -p clwd-runtime --features microvm
cargo build -p clwd-agent --target x86_64-unknown-linux-musl
cargo test --workspace
cargo clippy --workspace --all-features -- -D warnings
cargo fmt --all
```

`clwd-agent` is excluded from `default-members` — it is musl-only and must
be built with `-p` and an explicit target. Do not add it back to the default set.

The microvm backend requires Linux with KVM. On macOS it will not build or run;
use a Linux VM.

## Crates

| Crate | Role |
| --- | --- |
| `clwd-cli` | binary `clwd`: `init`, `run`, `dev`, `build`, `publish`, `logs`, `secrets`, `mcp` |
| `clwd-sdk` | what app code links against; WIT bindings |
| `clwd-runtime` | the `Sandbox` trait + `wasm` and `microvm` backends |
| `clwd-agent` | musl init (PID 1) that runs *inside* the microVM guest |

"Agent" is overloaded in this repo. `clwd-agent` is the in-guest init process —
unrelated to the AI agents that generate app code (`templates/`, `crates/sdk/wit/`, the
untrusted-code assumption below). When the sense is not obvious from context, say
which.

## Architectural rules

**The `Sandbox` trait is the only abstraction callers see.** Both backends
implement it. Do not add methods that only make sense for one backend, and do not
expose backend-specific types (`wasmtime::Store`, Firecracker socket handles) in
the public API. If a feature genuinely cannot be expressed through `Sandbox`,
raise it rather than widening the trait with a backend-shaped method.

**Backends are Cargo features, and the gating must stay strict.** `wasm` pulls in
`wasmtime`; `microvm` pulls in `hyper`/`hyperlocal`/`tokio-vsock`/etc. All are
`optional = true`. `clwd-cli` must never pull in `wasmtime` just to print logs —
if a change makes the CLI's dependency tree grow a runtime backend, that change is
wrong.

**Add shared dependencies to `[workspace.dependencies]` in the root manifest,**
then reference them as `foo = { workspace = true }` in crate manifests. Do not
pin versions per-crate.

**This workspace must build standalone from a clean clone.** Never add a path or
git dependency on another repository. Downstream code depends on this workspace,
never the reverse, and a stray path dependency defeats that silently — it works locally and breaks CI. CI enforces this with
`cargo-deny` plus a clean-clone build job.

## Backend notes

**wasm** — component model, not core modules. `wasmtime-wasi-http` for the HTTP
surface. Use epoch interruption for CPU limits, not fuel.

**microvm** — there is no well-maintained `firecracker-rs` wrapper; drive the
Firecracker API socket directly with `hyper` + `hyperlocal`. `jailer` provides the
outer sandbox. `tokio-vsock` for host↔guest control. Snapshot-restore is a day-one
design constraint, not a later optimization — it is what gets cold start under
~150ms, and retrofitting it means reworking how VM state is created. Write new
microvm code as if snapshots already exist.

## Product surface

`templates/` and `crates/sdk/wit/` are read by agents generating apps, not just by humans.
Treat changes there as product changes: breaking a template breaks generated code
in the field. `clwd-sdk` publishes to crates.io on its own version cadence and is
consumed downstream through tag-pinned git dependencies, so a breaking SDK change
needs a version bump and a tag, not just a merge.

### Seam pins are tags, never Releases

A tag is a ref. A Release is an announcement. The pins downstream code consumes
are refs, so they are annotated tags and nothing else.

`gh release create` is reserved for MVP and after. This matters more than it
sounds: the repository is private today, so nobody outside the org sees a
Release — but the moment it flips to public at MVP, every Release ever cut
becomes visible retroactively. The first release people see should be the one
you meant them to see, not an internal seam pin sitting at the top of the list
marked "Latest" with auto-generated notes.

**The workflow enforces this rather than trusting anyone to remember it.**
`cd.yml` originally cut a Release on every `v*` tag push, which is how `v0.0.1`
got one. A rule written here and disobeyed by our own CI would be worse than no
rule, so cutting a Release is now `workflow_dispatch` only — a deliberate act
someone chooses, which is exactly the tag-versus-Release distinction. Pushing a
seam pin builds the tag as a check and stops there.

Bumping a pin: move forward to a new tag rather than moving a published one.
`clwd-sdk` and `clwd-bundle` are pinned together at the same tag and move
together.

## A check that passes without checking

Four times now, a check in this repository has been green while the thing it
named was broken. The instances differ; the shape does not.

| Where | The check | Why it passed anyway |
| --- | --- | --- |
| #28 | `clwd build` warns on a bad release profile | `toml::Value`'s `FromStr` parses one *value*, so a manifest starting `[profile.release]` was read as an array and rejected — the function returned zero warnings for every input |
| #72 | the conformance suite, 35/35 | it ran against the committed `app.wasm`, so it stayed green while the fixture's source no longer compiled |
| #72 | an optimiser comparison | it skipped itself when `wasm-tools` was absent, which is precisely the CI environment it mattered in |
| #29 | `assert_eq!(ns.key("note"), "u/alice/note")` | it asserted what the function *builds*, not what the store *accepts* — `/` is not in the published key grammar, so every call failed at runtime |

The general form, and the one worth remembering:

> **A pure-function test of a value another system validates is not a test of
> the integration.** Neither is a test that runs against a committed artifact
> instead of a fresh one, or one that skips itself where it counts.

What each of these needed was the same: exercise the property against the thing
that judges it. `ns.key()` should be checked against the grammar
`crates/sdk/wit/deps/app-data/app-data.wit` publishes, not against a string someone chose.
The conformance suite needed something that rebuilds the fixture. The optimiser
comparison needed the tool installed rather than a conditional skip.

Two habits that catch this cheaply:

- **Mutate the code and watch the test fail.** A test that has never failed has
  never been shown to work. Every one of the four above passes its own
  assertions perfectly.
- **Ask what would have to be true for this to pass while the feature is
  broken.** If the answer is short, the check is aimed at the wrong thing.

## Untrusted code assumption

Everything running in a sandbox was probably written by an agent and is being run
by a non-engineer against real company data. When in doubt about a
capability — filesystem access, network egress, host resource limits — default to
denying it and make it explicit rather than inheriting whatever the runtime allows
by default.

## `gh pr edit` and Projects (classic)

`gh pr edit` can fail against these repositories with

```
GraphQL: Projects (classic) is being deprecated in favor of the new Projects
experience ... (repository.pullRequest.projectCards)
```

The important part is that it **fails silently from the caller's point of
view**: the error goes to stderr, the exit status is easy to miss in a script,
and the PR body is left unchanged. Anyone who does not re-read the PR afterwards
will believe an edit landed that did not.

Use the REST endpoint instead, which does not touch the deprecated field:

```bash
gh api -X PATCH repos/clwdrun/<repo>/pulls/<N> -F body=@body.md
```

Either way, read the body back after editing. The same applies to `gh issue
edit`.
