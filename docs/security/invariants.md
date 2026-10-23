# Isolation and safety invariants

Each invariant below is true of the code in this repository today, and each
names what enforces it: a test, a `just` recipe, or a CI workflow step. The
threat model, [threat-model.md](threat-model.md), cites these IDs.

**How this stays honest.** Every test, recipe and step that enforces an
invariant says so with a line `Invariant: ID.` in its doc comment or comment.
`just invariants`, part of `just check` and the CI gate, fails when an
invariant here has no such reference, or when a reference names an ID this
file does not define. Deleting the last test of an invariant, or citing one
that does not exist, turns the gate red.

**IDs are permanent.** An ID is never renumbered or reused. A retired
invariant keeps its heading, says it is retired and why, and keeps a
reference so the check still passes. New invariants take the next number in
their group.

Runtime invariants hold for the `wasm` backend, the only backend that exists.
Where a runtime invariant is also a backend conformance requirement, its
`R<n>.<n>` id from `clwd_runtime::conformance::REQUIREMENTS` is given; the
conformance suite (`just test-conformance`) has one test per requirement.

Tests are named by function; `rg 'fn <name>'` finds each one.

## Runtime (`clwd-runtime`, `wasm` backend)

### RT-1 · Each request runs in a fresh instance

Every `invoke` builds a new wasmtime `Store` and instantiates the component
into it, so no linear memory, global or table survives from one request to
the next, including between two requests from the same caller.

Enforced by `guest_state_never_survives_into_the_next_request`
(`crates/runtime/tests/wasm_lifecycle.rs`).

### RT-2 · An instance serves one caller, fixed by the host at spawn

The caller's identity is an argument to `spawn`, supplied by the host. An
instance observes exactly that identity for its whole life, two callers get
two instances, interleaved or concurrent calls on two handles never cross
identities, and nothing an `invoke` carries can change it. The guest
interface (`crates/sdk/wit/deps/identity/identity.wit`) has no function that
constructs or alters a principal.

Conformance R1.1–R1.6. Enforced by `r1_1` … `r1_6`
(`crates/runtime/tests/conformance.rs`) and by
`every_instance_observes_exactly_one_caller`,
`two_callers_never_share_an_instance` and
`a_handle_identity_is_read_only_and_fixed_at_spawn`
(`crates/runtime/tests/isolation.rs`).

### RT-3 · An anonymous caller is anonymous

An anonymous caller reaches the guest as `anonymous`, never as a user with
empty fields.

Conformance R1.8. Enforced by `r1_8`.

### RT-4 · Authorization fails closed

`authorize` and `authorize-many` return `unavailable` while no policy engine
answers them. They never return `allowed`.

Conformance R1.7. Enforced by `r1_7`.

### RT-5 · A guest is stopped when its CPU budget is spent

`capabilities.limits.cpu_ms`, sealed in the bundle, is enforced by epoch
interruption from a dedicated OS thread. A guest that never yields is
preempted with `runtime.cpu_budget_exceeded`. The budget applies when the
caller sets no deadline, and composes with a caller's wall-clock deadline:
whichever expires first ends the call. One guest spending its budget does not
slow a concurrent caller, and more spinning guests than executor threads are
all preempted while an unrelated caller is still served. What `cpu_ms`
measures is a known limitation; see the threat model.

Conformance R4.1, R4.2, R4.3, R4.5, R4.6. Enforced by `r4_1`, `r4_2`, `r4_3`,
`r4_5`, `r4_6`, `spinning_guests_cannot_starve_the_epoch_ticker`
(`crates/runtime/tests/wasm_starvation.rs`) and
`an_epoch_interrupt_maps_to_cpu_budget_exceeded`
(`crates/runtime/src/wasm/invoke.rs`).

### RT-6 · Linear memory is capped at the sealed budget

Growing linear memory past `capabilities.limits.memory_mb` stops the
invocation with `runtime.memory_limit_exceeded`, carrying the sealed byte
count as the limit. The guest is stopped rather than handed a failed
`memory.grow`.

Conformance R4.4. Enforced by `r4_4` and
`a_memory_denial_maps_to_memory_limit_exceeded`.

### RT-7 · Table growth is capped

A table may hold at most 100,000 elements per instance; growth past that
stops the invocation with `runtime.table_limit_exceeded`. The test drives the
real limiter and the error mapping, not a guest instruction: no guest built
with stable Rust can grow a table to the cap.

Enforced by `the_table_cap_maps_to_table_limit_exceeded`.

### RT-8 · A response body is capped at 16 MiB

A response body past `MAX_RESPONSE_BODY` (16 MiB) fails the invocation with
`runtime.response_too_large`. It is never truncated. The body is read while
the guest runs, so a large body cannot stall the guest until its deadline.

Enforced by `a_response_body_past_the_cap_is_refused`.

### RT-9 · Guest output is bounded, attributed, and drops are recorded

Each invocation's stdout and stderr are each capped at 256 KiB. Past the cap,
the rest of that invocation's output on the stream is dropped, and a line
records that a drop occurred. Every log line carries its invocation's
request id.

Conformance R7.1, R7.2. Enforced by `r7_1`, `r7_2` and
`output_past_the_cap_is_dropped_and_counted_once`
(`crates/runtime/src/wasm/logs.rs`).

### RT-10 · Outbound HTTP is denied unless the manifest grants the host

An outbound request is checked against `capabilities.egress` before it leaves
the host process. An empty list denies every host. An entry is an exact host
or a `*.suffix` wildcard, which matches names strictly under the suffix but
not the suffix itself; matching ignores ASCII case. A denied request fails
with the generic `HttpRequestDenied`, which does not name the host. The check
is by the name the guest asks for, not the address it resolves to.

Enforced by `an_empty_egress_list_denies_every_host`,
`an_exact_host_matches_only_itself`,
`a_suffix_wildcard_matches_subdomains_but_not_the_apex`,
`matching_is_case_insensitive` and
`a_disallowed_host_is_denied_before_the_request_leaves`
(`crates/runtime/src/wasm/capability.rs`).

### RT-11 · App data is denied unless the manifest grants storage

With an empty `capabilities.storage_namespaces`, every app-data operation is
refused. A non-empty list grants the app's whole store; the namespace strings
themselves are not interpreted.

Enforced by `an_empty_storage_grant_denies_all_app_data`
(`crates/runtime/src/wasm/capability.rs`) and
`an_ungranted_app_data_operation_is_denied`
(`crates/runtime/src/wasm/host.rs`).

### RT-12 · App data is partitioned by the app the host names

Data is keyed by the app id the host passes at spawn, never by the bundle
digest, so two apps built from one artifact do not share data. A host that
requires an app id refuses a spawn that names none.

Conformance R6.4, R6.7. Enforced by `r6_4`, `r6_7` and
`app_id_required_refuses_a_spawn_that_names_no_app`.

### RT-13 · A guest reads only secrets both the manifest and the store grant

A secret is readable only when the manifest declares it and the host's store
holds it for that app. Otherwise it is absent, and absent looks the same
whether the secret does not exist or is not this app's.

Conformance R5.1–R5.3. Enforced by `r5_1`, `r5_2`, `r5_3` and
`a_guest_sees_only_the_secrets_its_manifest_declared`
(`crates/cli/tests/secrets.rs`).

### RT-14 · A crash stays inside its invocation

A trapping guest reports `runtime.trap` with a message, never a budget code,
and one caller's crash affects neither another caller of the same bundle nor
the compiled-component cache.

Conformance R3.1–R3.3. Enforced by `r3_1`, `r3_2`, `r3_3` and
`a_trap_does_not_poison_the_cache_for_other_callers`.

### RT-15 · A guest has no ambient authority

The WASI context gives a guest no filesystem, environment variables,
arguments, sockets or name lookup. The host still links those interfaces, so
a guest that imports them is instantiated and every call is refused.
Everything else a guest reaches is a clwd interface governed by RT-10 to
RT-13.

Enforced by `a_guest_has_no_ambient_authority`
(`crates/runtime/tests/wasm_lifecycle.rs`).

### RT-16 · wasmtime is built with only the reviewed features

Default features are off for `wasmtime`, `wasmtime-wasi` and
`wasmtime-wasi-http`. Across the workspace, with every feature on, the
enabled set is exactly: `addr2line`, `async`, `backtrace`, `component-model`,
`cranelift`, `parallel-compilation`, `runtime`, `std`,
`wasmtime-jit-icache-coherence`, `wasmtime-unwinder`; `wasmtime-wasi` `p2`;
`wasmtime-wasi-http` `p2` and `default-send-request`. Not built: the async
component model, GC, threads, the pooling allocator, WASIp1, WASIp3.

Enforced by `just wasmtime-features` and its CI step.

### RT-17 · A bundle is verified every time it is spawned

`spawn` decodes the bundle against the digest it was asked for, so every
`bundle.*` rule (BUN-1 to BUN-7) is applied before wasmtime sees any bytes,
and the failing code is returned unchanged. A core module is refused, and so
is a component built against an interface version the host no longer
provides, at spawn rather than mid-request.

Conformance R2.4. Enforced by `r2_4`,
`a_bundle_failure_keeps_its_own_code_verbatim`
(`crates/runtime/tests/lifecycle.rs`), `a_core_module_is_rejected_at_ingest`
and `a_component_built_against_identity_0_1_is_refused_at_spawn`.

### RT-18 · Stopping an instance releases it

`stop` removes the instance, including its copy of the app's secrets.

Enforced by `stop_removes_the_instance`.

### RT-19 · Every failure has a stable code and an honest `retryable`

Every runtime error variant has exactly one owned `runtime.*` code, and
whether it is retryable agrees with the published catalogue.

Enforced by `owned_codes_are_exhaustive_over_variants` and
`retryable_agrees_with_the_catalogue` (`crates/runtime/src/error.rs`).

## Bundle format (`clwd-bundle`)

### BUN-1 · Encoding is canonical

One source tree produces one byte sequence, on any platform: packing twice is
byte-identical, encode–decode–encode is a fixed point, member order and
non-executable metadata do not matter, and paths are NFC-normalized before
hashing. The committed fixture in `docs/spec/fixtures` is reproduced exactly.

Enforced by `packing_the_same_tree_twice_is_byte_identical`,
`encode_decode_encode_is_a_fixed_point`, `member_order_does_not_affect_output`,
`source_metadata_beyond_the_executable_bit_does_not_affect_output`,
`paths_are_normalized_so_the_same_tree_hashes_alike_on_any_platform`
(`crates/bundle/tests/properties.rs`) and `matches_the_committed_fixture`
(`crates/bundle/tests/conformance.rs`).

### BUN-2 · The digest covers every byte and is checked first

The bundle digest is BLAKE3 over the whole canonical archive. Flipping any
byte changes it. When the caller supplies an expected digest, it is compared
before the archive is parsed, and each member's own digest is checked too.

Enforced by `a_flipped_byte_anywhere_changes_the_bundle_digest`,
`expected_digest_is_checked_before_the_archive_is_parsed`,
`decode_rejects_a_flipped_byte_in_a_member`, `digest_mismatch` and
`member_digest_mismatch` (`crates/bundle/tests/`).

### BUN-3 · Member paths cannot escape or alias

A member path is refused when it is empty, absolute, has a trailing slash,
an empty segment, a `.` or `..` segment or a control character, is not NFC,
is longer than 100 bytes, or appears twice.

Enforced by `path_invalid`, `path_not_nfc`, `path_too_long` and
`path_duplicate` (`crates/bundle/tests/rejections.rs`).

### BUN-4 · Members are regular files only

Any tar entry type other than a regular file (a link, a directory, a device)
is refused.

Enforced by `entry_type_invalid`.

### BUN-5 · Members are exactly what the manifest lists

A member the manifest does not list, a listed member that is absent, a size
that differs from the listed size, or a size at or past 8 GiB is refused.

Enforced by `member_unlisted`, `member_missing`, `member_size_mismatch` and
`member_too_large`.

### BUN-6 · The manifest is strict

An unknown key, at the top level or under `capabilities`, is refused, as are
a missing field, a field of the wrong type or out of range, an unsupported
schema or capabilities version, and a manifest not in canonical form.
Nothing unrecognised is ignored.

Enforced by `unknown_key`, `capability_unknown_key`, `field_missing`,
`field_type_invalid`, `field_value_invalid`, `schema_version_unsupported`,
`capabilities_version_unsupported` and `manifest_not_canonical`.

### BUN-7 · The entrypoint is a component

An entrypoint that is not a WebAssembly component, including a core module,
is refused.

Enforced by `wasm_not_component` and `wasm_preamble_invalid`.

### BUN-8 · Every rejection is tested

Every code in the bundle error catalogue has a test that produces it.

Enforced by `every_catalogue_entry_has_a_test`.

## CLI (`clwd-cli`)

### CLI-1 · The platform token is read from the environment or an owner-only file

The token comes from `CLWD_TOKEN` or a `credentials` file beside the local
secret store. On Unix, a credentials file readable by anyone but its owner is
refused.

Enforced by `an_owner_only_file_yields_the_token` and
`a_world_readable_token_file_is_refused` (`crates/cli/src/platform/config.rs`).

### CLI-2 · The platform token never appears in output

Not in a rendered result, an error, a `Debug` rendering, an MCP tool result
or progress message, or the MCP server's stderr.

Enforced by `a_token_redacts_itself_in_debug`,
`a_rejected_token_is_not_repeated_in_the_error`,
`the_token_never_appears_in_a_rendered_result` and
`every_tool_runs_and_the_token_never_appears` (`crates/cli/tests/mcp.rs`).

### CLI-3 · A subprocess gets no `CLWD_*` variable and no stdin

`build::process::command` is the only place the CLI constructs a subprocess.
Every child it makes has stdin closed and every `CLWD_*` variable removed, so
cargo, and the build scripts and proc macros it runs, hold neither the
platform token nor the MCP protocol stream.

Enforced by `a_noisy_build_never_corrupts_stdout` (a build script prints its
whole environment), `a_subprocess_never_inherits_the_protocol_stream` (a
`cargo` on `PATH` reports its stdin) and `just spawn-confined` with its CI
step.

### CLI-4 · App log text is rendered inert

Control characters, escape sequences and bidirectional formatting characters
in app output are shown as visible escapes, never acted on by a terminal, and
a backslash is doubled so a written escape stays distinguishable. `clwd dev`
prints app lines exactly as `clwd logs` does.

Enforced by `hostile_text_renders_inert`,
`a_backslash_is_doubled_so_a_written_escape_is_distinguishable`
(`crates/cli/src/commands/logs.rs`),
`hostile_text_is_inert_for_people_and_exact_for_programs`
(`crates/cli/tests/logs.rs`) and
`the_dev_server_prints_app_lines_as_clwd_logs_does`.

### CLI-5 · The local secret store is owner-only and per app

On Unix the store file is created `0600` in a `0700` directory, and a store
readable by others is refused. One app's secrets are invisible to another.

Enforced by `the_file_is_owner_only_and_so_is_its_directory`,
`a_world_readable_store_is_refused`, `one_apps_secrets_are_invisible_to_another`
(`crates/cli/src/secrets/store.rs`) and
`the_store_the_binary_creates_is_owner_only` (`crates/cli/tests/secrets.rs`).

### CLI-6 · A secret's value is never printed

Listings carry names only, a change reports a size but not the value, no
error message echoes a value, and a secret value's `Debug` rendering shows
its length and nothing else.

Enforced by `a_stored_secret_never_appears_where_it_should_not`,
`no_error_message_echoes_a_value`, `debug_never_renders_the_value`,
`the_json_listing_carries_names_only` and
`a_change_reports_a_size_but_never_a_value`.

### CLI-7 · Two deploys of one project never run at once

A deploy holds an exclusive lock in the project's `target/` directory; a
second deploy of the same project is refused with `cli.deploy_in_progress`,
which is retryable. The CLI command and the MCP tool share the lock.

Enforced by `a_second_concurrent_deploy_is_refused`.

### CLI-8 · A host label is checked before anything is built or sent

A deploy reads the host label only from the project's `clwd.toml`, and
refuses a missing label or one that is not an RFC 1123 DNS label before any
build or request. The MCP `deploy` tool also requires `confirm_host_label`
to equal it, and refuses a mismatch before building. That a label, once
claimed, is permanent is the platform's guarantee, not something this
repository can test.

Enforced by `a_host_label_is_an_rfc_1123_label`
(`crates/cli/src/build/manifest.rs`),
`a_missing_host_label_fails_before_any_build_or_request`,
`an_invalid_host_label_fails_before_any_build_or_request`
(`crates/cli/tests/deploy.rs`) and `every_tool_runs_and_the_token_never_appears`.

### CLI-9 · A failure never writes to stdout

A failing command writes its error, as text or as the JSON envelope, to
stderr and nothing to stdout. Checked for build failures, usage errors and
`clwd run` failures, in both renderers.

Enforced by `failures_never_write_to_stdout` and
`run_failures_never_write_to_stdout`.

### CLI-10 · The CLI never asks a question

Exactly two places read stdin, both in `main.rs`: the MCP transport, and
`clwd secrets set`, which reads the value as data. No command prints a
question and waits for an answer.

Enforced by `just stdin-readers` with its CI step, and
`init_with_stdin_closed_scaffolds_and_prints_the_next_commands`.

### CLI-11 · The token crosses a network only over https

A platform URL must be `https`, or plain `http` to a loopback host: an
address in `127.0.0.0/8`, `::1`, or exactly `localhost`, judged from the
parsed host. Any other plain-http URL is refused with
`cli.insecure_platform_url` before anything is built or sent. Every HTTP
client the CLI builds comes from `PlatformUrl::client`, which follows no
redirect (a redirect is reported as an error naming where it pointed) and,
for a platform that is not loopback, refuses to send any request over plain
http.

Enforced by `plain_http_is_accepted_only_for_a_loopback_host`,
`a_client_for_a_remote_platform_never_sends_plain_http`
(`crates/cli/src/platform/mod.rs`), `a_redirect_is_reported_and_not_followed`
(`crates/cli/src/platform/client.rs`),
`a_plain_http_platform_url_is_refused_before_any_build_or_request`
(`crates/cli/tests/deploy.rs`), and `just platform-client` with its CI step.

## MCP server (`clwd mcp serve`)

### MCP-1 · stdout carries protocol messages only

The CLI's library never prints. Exactly two places take stdout, both in
`main.rs`: the CLI renderer and the MCP transport. Under `clwd mcp serve`, a
build that floods stdout and stderr, and a tool that panics, leave stdout as
nothing but JSON-RPC.

Enforced by `a_noisy_build_never_corrupts_stdout`,
`a_panicking_tool_never_reaches_stdout` (`crates/cli/tests/mcp_panic.rs`),
`rendering_never_reaches_for_stdout` (`crates/cli/tests/lib_api.rs`),
`just lib-silent`, `just stdout-owners` and its CI step.

### MCP-2 · Untrusted text comes back as data

App logs and build output reach a client only as `text` fields inside
structured lines, with a `source` set by the platform or the server. The text
content block is the structured value serialized as JSON with no prose
around it, and control and bidirectional characters in it are written as
`\uXXXX` escapes.

Enforced by `log_text_stays_data` and
`inert_json_escapes_what_a_display_would_act_on_and_parses_back`.

### MCP-3 · Tools act only inside the workspace root

A path argument is refused when it is absolute or when it resolves, through
`..` or any symlink, outside the root.

Enforced by `paths_outside_the_root_are_refused`.

### MCP-4 · The tool set is fixed and every hint is explicit

The server exposes exactly six tools (`list_templates`, `init`, `build`,
`get_app`, `deploy`, `logs`), each with every annotation hint stated, and
`deploy` marked destructive. No tool sets, reads or lists a secret.

Enforced by `tools_list_declares_six_tools_with_explicit_hints`.

### MCP-5 · A cancelled build leaves nothing behind

Cancelling `build` stops cargo and its process group on Unix and leaves no
bundle, whole or partial.

Enforced by `a_cancelled_build_stops_and_leaves_no_bundle`.

### MCP-6 · The test-only tool never ships

The panicking tool used to test panic handling exists only behind a
test-only feature and is absent from the release binary.

Enforced by `just mcp-release-bare` and its CI step.

## SDK and templates

### SDK-1 · Pages are sent as what they are

A page built with the SDK's `Response::html` is sent as
`text/html; charset=utf-8` with `X-Content-Type-Options: nosniff`, so a
browser does not reinterpret it.

Enforced by `hello_greets_the_caller_by_verified_email`,
`notes_lists_an_empty_page_with_a_form_that_posts`
(`crates/cli/src/commands/dev/served/templates.rs`) and
`init_then_dev_serves_the_scaffold` (`crates/cli/tests/init.rs`).

### SDK-2 · Text placed in HTML is escaped

`Html::text` escapes every character that can end an HTML context, and the
templates' pages escape caller-supplied and stored text.

Enforced by `text_escapes_every_character_that_can_end_a_context`
(`crates/sdk/src/html.rs`), `hello_escapes_an_email_that_contains_markup` and
`notes_escapes_a_stored_note_that_contains_markup`.

### SDK-3 · Per-caller data cannot collide

`data::for_caller` keys by the caller's subject, so two callers never share a
key, a caller's key never collides with an app-wide one, an anonymous caller
has no private namespace, and a subject outside the key grammar is refused
rather than reshaped.

Enforced by `two_callers_do_not_collide`,
`a_caller_key_cannot_collide_with_an_app_wide_key`,
`an_anonymous_caller_has_no_private_namespace`,
`a_subject_outside_the_grammar_is_refused_rather_than_reshaped`
(`crates/sdk/src/data.rs`) and `notes_are_private_to_the_caller_who_wrote_them`.

### SDK-4 · An SDK error message carries no caller data

Enforced by `an_error_message_cannot_carry_caller_supplied_data`
(`crates/sdk/tests/sample_app.rs`).

### TPL-1 · Templates contain no authentication and trust only `caller()`

A source scan of every template finds none of `cookie`, `authorization`,
`bearer`, `session`, `login`, `password` or `jwt` in code, and no string
literal naming an identity field (`user`, `email`, `subject` and the like)
that a template could read from a request. Identity comes from
`clwd_sdk::caller()`. A scan is a guard against the obvious, not a proof.

Enforced by `no_template_contains_auth_code` and
`no_template_reads_identity_from_the_request` (`crates/cli/tests/templates.rs`).

### TPL-2 · Template responses go through `html` or `text`

Enforced by `every_body_goes_through_html_or_text`.

### TPL-3 · Templates are granted no egress and no secrets

Enforced by `no_template_is_granted_egress_or_secrets`.

## Local development (`clwd dev`, `clwd run`)

### DEV-1 · The dev server listens on loopback only

`clwd dev` binds `127.0.0.1`, whatever port it is given.

Enforced by `the_dev_server_listens_on_loopback_only`
(`crates/cli/src/commands/dev/mod.rs`).

### DEV-2 · The local caller is marked local and cannot ship

`clwd dev` and `clwd run` invoke the app as a fixed local caller (subject
`local`, org `local`), which is visibly not a deployed identity. It is an argument
the CLI passes at spawn, not part of the bundle: the bundle manifest has no
identity field and refuses unknown keys (BUN-6), so no build can carry it.
A deployed app's caller is supplied by the platform.

Enforced by `the_local_caller_is_not_mistakable_for_a_production_one`
(`crates/cli/tests/run.rs`).

## Supply chain and release

### SC-1 · Dependencies pass `cargo deny` on every change

Advisories, licenses, bans and sources are checked in the CI gate and before
a tag is accepted.

Enforced by `just deny` and the CI `deny` step.

### SC-2 · Advisories are checked daily, and a failure opens an issue

`advisories.yml` runs `cargo deny check advisories` against `main` every day.
On failure it comments on the open tracking issue, or opens one.

Enforced by the `advisories.yml` step that opens or updates the issue.

### SC-3 · No secret is committed

gitleaks scans the full history in CI and the staged changes in the
pre-commit hook.

Enforced by `just secrets` and the CI `secrets` step.

### SC-4 · CI tools are verified before they run

gitleaks and cargo-deny are downloaded at pinned versions and their sha256
is checked before they are unpacked.

Enforced by the CI `install gitleaks` and `install cargo-deny` steps.

### SC-5 · A tag is accepted only from green `main`

A pushed tag is refused unless its commit is on `main`, `ci` passed on that
exact commit, and a fresh advisories check passes.

Enforced by the `cd.yml` gate steps.

### SC-6 · Pushing a tag never cuts a Release

A GitHub Release is created only by a manual `workflow_dispatch` naming an
existing tag.

Enforced by the `release` and `cut-release` jobs in `cd.yml`, which run only
on `workflow_dispatch`.

### SC-7 · The bundle specification and its fixtures are sealed

`docs/spec` and its fixtures match their recorded sha256 sums.

Enforced by `just spec-intact` and its CI step.
