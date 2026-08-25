# clwd bundle format, version 1

**Status:** frozen. Changes to this document after freeze require a new
`schema_version`.

A bundle is the unit clwd deploys, addresses and runs. This document defines it
completely enough that an independent implementation in any language produces
byte-identical archives from the same inputs, and reaches the same accept or
reject decision — with the same error code — on the same bytes.

Six things consume this format: `BundleRef` in the `Sandbox` trait (#15), the
build pipeline (#28), the capability manifest (#33), the runtime backends, and
across the repository seam the platform's builder ingest and its content-digest
idempotency key. Where this document and an implementation disagree, this
document is correct and the implementation is a bug.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be interpreted
as described in RFC 2119.

## 1. Terminology

**Bundle** — a canonical tar stream containing a manifest and one or more
members.

**Member** — one regular file inside the bundle, other than the manifest.

**Manifest** — the file `clwd.toml` at the root of the archive.

**Canonical tar stream** — the byte sequence defined in §3. This is the *only*
representation that is a bundle. An archive that decodes to the same logical
content but differs in any byte is not a bundle; it is rejected.

**Member digest** — BLAKE3 over one member's contents. Listed in the manifest.
Detects tampering with a single member without unpacking the whole archive.

**Bundle digest** — BLAKE3 over the entire canonical tar stream. This is the
bundle's identity, and the value carried by `BundleRef`.

## 2. The artifact

A bundle is a single file, conventionally named `*.clwd`. Its contents are the
canonical tar stream and nothing else.

### 2.1 Compression is outside the format

Bundles are **stored compressed and addressed uncompressed**. A storage layer
MAY hold a bundle zstd-compressed, conventionally `*.clwd.zst`. Compression is
never inside the digest.

This is deliberate. If compression were inside the digest, a zstd version bump
or a level change would silently rewrite every digest in the system: every
content-digest idempotency key would miss, and every stored artifact would be
orphaned behind an address nothing computes any more. Compression is a transport
and storage detail, and it stays one.

An implementation of this specification reads and writes the uncompressed
canonical stream. Compressing and decompressing is the caller's business.

## 3. Canonical tar serialization

The stream is a sequence of 512-byte blocks. Every rule below is normative;
there is nothing left to infer.

### 3.1 What is serialized

The input is a set of *(path, contents, executable)* triples. Nothing else about
the source — timestamps, ownership, the rest of the mode bits, extended
attributes, ACLs — is carried, and no two inputs differing only in those
respects produce different output.

- **Directory entries MUST NOT be emitted.** Directories are implied by member
  paths. An empty directory in the source produces no entry: a bundle cannot
  represent one.
- **Only regular files.** An encoder walking a source tree MUST reject a
  symbolic link, hard link, device node, FIFO or socket rather than following,
  dereferencing or recording it. Following a symlink out of the source tree is a
  path-escape vulnerability, and recording one is forbidden by the wire format
  in §3.4.
- The archive MUST contain the manifest and at least one member.

### 3.2 Paths

A member path is a slash-separated relative path. After normalization it MUST
satisfy all of:

| Rule | Rejection |
| --- | --- |
| Valid UTF-8 | `bundle.path_invalid` |
| Non-empty | `bundle.path_invalid` |
| No leading `/` (not absolute) | `bundle.path_invalid` |
| No trailing `/` | `bundle.path_invalid` |
| No empty segment (no `//`) | `bundle.path_invalid` |
| No segment equal to `.` or `..` | `bundle.path_invalid` |
| No codepoint in U+0000–U+001F or U+007F | `bundle.path_invalid` |
| In Unicode Normalization Form C | `bundle.path_not_nfc` |
| At most 100 bytes when UTF-8 encoded | `bundle.path_too_long` |
| Distinct from every other path in the bundle | `bundle.path_duplicate` |

Separators are `/` (U+002F) only. A backslash is an ordinary character in a path
segment, not a separator.

**Normalization is an encoder responsibility, rejection is a decoder
responsibility.** An encoder MUST convert each path to NFC before use. A decoder
MUST reject any path that is not already NFC. This asymmetry is what makes the
cross-platform digest stable: macOS hands filenames to a process in NFD, Linux
in NFC, and without normalizing on the way in, the same source tree would
produce two different bundle digests on two machines.

If two distinct source paths normalize to the same NFC form, the encoder MUST
reject the input with `bundle.path_duplicate` rather than silently dropping one.

**100 bytes, not 256.** USTAR can store a path of up to 256 bytes by splitting
it across a 155-byte `prefix` field and a 100-byte `name` field, and the split
point is not determined by the format. Two conforming implementations may split
the same path differently, producing byte-different archives with identical
logical content — which means two different bundle digests for the same
artifact. In an identity layer that is not a cosmetic difference: the platform's
content-digest idempotency key would miss, and one artifact would be stored
twice as two unrelated things. Capping the path removes the field, and with it
the ambiguity. §3.4 restates this as a wire constraint so it is verifiable on
decode rather than merely observed on encode.

### 3.3 Ordering

Entries are sorted **byte-wise ascending** over the UTF-8 encoding of the
normalized path. Not by codepoint value, not by any locale collation, not by
path component. The manifest is sorted alongside the members by the same rule —
`clwd.toml` has no privileged position.

A decoder MUST reject an archive whose entries are not in this order with
`bundle.entries_not_sorted`.

### 3.4 Entry header

Every entry begins with one 512-byte header block. Every byte is specified.
Fields not listed carry their full length in NUL bytes.

| Offset | Length | Field | Value |
| --- | --- | --- | --- |
| 0 | 100 | `name` | path, UTF-8, NUL-padded to 100 |
| 100 | 8 | `mode` | `0000644\0` or `0000755\0` |
| 108 | 8 | `uid` | `0000000\0` |
| 116 | 8 | `gid` | `0000000\0` |
| 124 | 12 | `size` | 11 octal digits, zero-padded, then NUL |
| 136 | 12 | `mtime` | `00000000000\0` |
| 148 | 8 | `chksum` | see §3.4.2 |
| 156 | 1 | `typeflag` | `0x30` (ASCII `0`) |
| 157 | 100 | `linkname` | 100 NUL bytes |
| 257 | 6 | `magic` | `ustar\0` (`75 73 74 61 72 00`) |
| 263 | 2 | `version` | `00` (`30 30`) |
| 265 | 32 | `uname` | 32 NUL bytes |
| 297 | 32 | `gname` | 32 NUL bytes |
| 329 | 8 | `devmajor` | 8 NUL bytes |
| 337 | 8 | `devminor` | 8 NUL bytes |
| 345 | 155 | `prefix` | **155 NUL bytes** |
| 500 | 12 | — | 12 NUL bytes |

**`typeflag` MUST be `0x30`, not `0x00`.** Both mean "regular file" to every
reader, so a mistake here never surfaces as a parse failure — it surfaces as two
implementations computing different digests for identical content. Several tar
libraries leave the byte at NUL unless told otherwise.

**`prefix` MUST be 155 zero bytes.** A decoder MUST reject any entry with a
non-empty prefix with `bundle.prefix_not_empty`, without attempting to
reconstruct the path, and regardless of whether the reconstructed path would be
valid. Enforcing this on the wire rather than only on input means a hostile or
buggy encoder cannot hand a reader a split path that some implementations
reassemble and others do not.

`uname` and `gname` are empty — 32 NUL bytes, not the string `root`.

No PAX extended headers (`typeflag` `x` or `g`), no GNU long-name or long-link
entries (`typeflag` `L` or `K`), no sparse encoding, no extended attributes, no
ACLs. Because `typeflag` MUST be `0x30`, every one of these is rejected by a
single rule as `bundle.entry_type_invalid`, as are directory (`5`), symlink
(`2`), hard link (`1`), character device (`3`), block device (`4`) and FIFO (`6`)
entries.

#### 3.4.1 Mode

Exactly two values are permitted:

- `0000755\0` when the source file has any execute bit set (`mode & 0o111 != 0`)
- `0000644\0` otherwise

No other value is emitted, and a decoder MUST reject anything else with
`bundle.mode_invalid`. setuid, setgid and sticky bits are not representable.

#### 3.4.2 Checksum

The checksum is the unsigned sum of all 512 header bytes, with the eight bytes
of the `chksum` field itself treated as ASCII spaces (`0x20`) for the purpose of
the sum.

It is stored as **six octal digits, zero-padded, then NUL, then space**. Byte by
byte across the field's eight bytes:

| Offset | Value |
| --- | --- |
| 148-153 | six ASCII octal digits, `0x30`-`0x37`, left-padded with `0x30` |
| 154 | `0x00` |
| 155 | `0x20` |

A checksum of octal 7405 is therefore the eight bytes
`30 30 37 34 30 35 00 20`. The NUL precedes the space; the reverse order is a
different byte string. This document gives the field in hex rather than as a
quoted string, because a trailing space inside a code span is invisible and an
invisible byte in a canonicalization rule is a defect.

Confirmed against GNU tar 1.35 with `--format=ustar` by inspecting the bytes it
emits, not by reading its documentation.

Both halves of this are places implementations diverge. Some sum the field's
actual bytes instead of spaces, which makes the checksum unverifiable. Some
store seven octal digits followed by NUL and no trailing space, which parses
identically everywhere but is a different byte string and therefore a different
digest.

The checksum protects nothing in this format. BLAKE3 covers everything it covers
and covers it better, so determinism is the field's only remaining job. That is
exactly why it is pinned to the byte: which form is chosen matters far less than
that exactly one form is permitted.

A decoder MUST recompute the checksum and reject a mismatch with
`bundle.checksum_invalid`. A decoder MUST also reject a checksum field that is
correctly valued but incorrectly encoded, for the same reason it rejects a
non-empty prefix: one logical archive, one byte string.

#### 3.4.3 Size

`size` is 11 zero-padded octal digits followed by NUL. Base-256 encoding — the
GNU extension signalled by a high bit in the first byte — MUST be rejected with
`bundle.size_encoding_invalid`.

The field's ceiling is therefore `0o77777777777` = 8589934591 bytes, one byte
under 8 GiB. A member of 8 GiB or larger MUST be rejected with
`bundle.member_too_large` rather than encoded some other way. No realistic
bundle approaches this; it is stated because an unstated limit is where a format
grows an extension.

### 3.5 Entry data

The header block is followed by the member's contents, then NUL bytes padding to
the next 512-byte boundary.

A file of length 0 is followed by **no data block at all** — not a block of
zeros. The next header, or the end-of-archive marker, begins immediately.

Padding bytes MUST be NUL. A decoder MUST reject non-zero padding with
`bundle.padding_invalid`. Padding is inside the digest, so a reader that ignores
it accepts archives that differ from what it would itself produce.

### 3.6 End of archive

The stream ends with **exactly two** 512-byte blocks of NUL, and nothing after
them.

**Both trailing blocks are inside the bundle digest.** This is stated explicitly
because it is precisely the kind of boundary two implementations disagree about
without either noticing, and the disagreement only surfaces as digests that fail
to match.

Many tar implementations pad the archive out to a blocking factor — GNU tar
defaults to 20 blocks, 10240 bytes. This format does **not**. The stream ends
after the second zero block. A decoder MUST reject trailing bytes, including
additional zero blocks, with `bundle.trailer_invalid`.

An archive is therefore always a whole number of 512-byte blocks, and always at
least four of them: one header and one trailer pair, for a single empty member.

## 4. Digests

Both digests are BLAKE3 with the default 32-byte output, unkeyed, no key
derivation context.

### 4.1 String form

```
blake3:<64 lowercase hexadecimal characters>
```

The prefix is exactly `blake3:`. Hexadecimal digits are `0`–`9` and `a`–`f`.
Uppercase is rejected, not normalized — accepting both would mean one digest has
two spellings, and a string-keyed cache would then hold two entries for one
artifact.

Anything else — a different algorithm prefix, no prefix, wrong length, a
non-hexadecimal character, uppercase, surrounding whitespace — is rejected with
`bundle.digest_malformed`.

### 4.2 Member digest

BLAKE3 over the member's contents exactly as stored: not the tar header, not the
NUL padding, not the contents of any other entry. A zero-length member has the
digest of the empty input,
`blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262`.

Member digests are what let a consumer detect tampering with one file without
unpacking or rehashing the whole archive.

### 4.3 Bundle digest and `BundleRef`

BLAKE3 over the entire canonical tar stream from the first byte of the first
header through the last byte of the second trailing zero block.

`BundleRef` is the bundle digest. It parses from and displays as the §4.1 string
form. A malformed reference is `bundle.digest_malformed`.

**The bundle digest MUST NOT appear inside `clwd.toml`.** The manifest is an
input to the digest, so a manifest containing it could not be constructed. It
lives only in metadata *about* a bundle: a registry row, an idempotency key, a
deployment record, a `BundleRef` passed to `Sandbox::start`.

## 5. The manifest

### 5.1 Location

The manifest is the entry at path `clwd.toml`. Exactly one MUST be present, at
the archive root. Its absence is `bundle.manifest_missing`.

The manifest is an ordinary entry: it is sorted with the members by §3.3, its
bytes are inside the bundle digest, and it is subject to every rule in §3.

**The manifest MUST NOT list itself in `[[members]]`.** It cannot: its own
digest is an input to its own contents. A manifest listing `clwd.toml` is
rejected with `bundle.manifest_self_listed`. Every other entry in the archive
MUST be listed.

### 5.2 Schema

```toml
schema_version = 1
runtime = "wasm"                # "wasm" | "microvm"

[app]
entrypoint = "app.wasm"

[[members]]
path = "app.wasm"
digest = "blake3:48d3a2e8a9b0cde6293ba7f68824a97d069d096f44335a9e86aee55238dcdcb8"
size = 8

[capabilities]
version = 1
egress = []
secrets = []
storage_namespaces = []

[capabilities.limits]
cpu_ms = 5000
memory_mb = 128
```

The file MUST be valid TOML, UTF-8, without a byte-order mark. A parse failure
is `bundle.manifest_not_toml`.

| Key | Type | Required | Constraint | Absent |
| --- | --- | --- | --- | --- |
| `schema_version` | integer | yes | exactly `1` | `bundle.schema_version_missing` |
| `runtime` | string | yes | `"wasm"` or `"microvm"` | `bundle.field_missing` |
| `app.entrypoint` | string | yes | equals the `path` of exactly one member | `bundle.field_missing` |
| `members[].path` | string | yes | a §3.2 path present in the archive | `bundle.field_missing` |
| `members[].digest` | string | yes | §4.1 string form | `bundle.field_missing` |
| `members[].size` | integer | yes | `0 ≤ size < 8589934592` | `bundle.field_missing` |
| `capabilities.version` | integer | yes | exactly `1` | `bundle.field_missing` |
| `capabilities.egress` | array of string | yes | see §5.3 | `bundle.field_missing` |
| `capabilities.secrets` | array of string | yes | see §5.3 | `bundle.field_missing` |
| `capabilities.storage_namespaces` | array of string | yes | see §5.3 | `bundle.field_missing` |
| `capabilities.limits.cpu_ms` | integer | yes | `1 ≤ n ≤ 3600000` | `bundle.field_missing` |
| `capabilities.limits.memory_mb` | integer | yes | `1 ≤ n ≤ 65536` | `bundle.field_missing` |

`[app]`, `[capabilities]` and `[capabilities.limits]` are required tables;
`[[members]]` MUST have at least one element. **Every key above is required.**
The manifest has exactly one shape: no optional keys, and therefore no key whose
absence has to be assigned a meaning. A value of the wrong TOML type is
`bundle.field_type_invalid`. A value of the right type outside its constraint is
`bundle.field_value_invalid`.

`[[members]]` MUST be sorted by `path`, byte-wise ascending — the same order as
the archive entries — and MUST NOT contain a duplicate path.

The resource limits are required rather than defaulted. A limit that appears
when stated and silently becomes something else when omitted has the same defect
as a silently-ignored permission, and `[capabilities]` is the one subtree where
this format does not tolerate that.

### 5.3 Capability arrays

`egress`, `secrets` and `storage_namespaces` are **deny-by-default**: an empty
array means "none granted", and no value means "all". All three keys are
required, so a bundle granting nothing says so in three explicit empty arrays
rather than by omission. An absent grant and an empty grant would otherwise be
two spellings of one thing, which §5.4 does not permit.

Each element MUST be a non-empty string. Each array MUST be sorted byte-wise
ascending and MUST NOT contain duplicates; otherwise
`bundle.field_value_invalid`. Sorting is required so that one grant set has one
spelling.

This version assigns **no meaning** to the element strings. A decoder MUST NOT
interpret, canonicalize or resolve them — no host parsing, no port defaulting,
no case folding. Their grammar and their enforcement belong to #33. A decoder
that guessed at their meaning would be a second, informal enforcement point
disagreeing with the real one.

### 5.4 Canonical serialization

The manifest's bytes are inside the bundle digest, so two producers emitting
semantically identical manifests with different formatting would produce
different bundle digests for the same application — a duplicate artifact and a
missed idempotency key, which is the same failure the §3.4 prefix rule exists to
prevent. Canonical form is therefore **normative for producers and verified by
decoders**, not merely recommended.

A manifest MUST be serialized as follows:

- UTF-8, no byte-order mark, LF line endings, exactly one trailing newline
- No comments, no blank line before the first key, exactly one blank line before
  each table header
- Key order exactly as listed in §5.2
- `key = value`, one space either side of `=`
- Strings as TOML basic strings in double quotes, escaping only `"`, `\` and
  codepoints below U+0020
- Integers in decimal, no underscores, no leading zeros, no leading `+`
- Empty arrays as `[]`; non-empty arrays on one line as `["a", "b"]`, elements
  in the sorted order required by §5.3
- Tables in the order: top-level keys, `[app]`, each `[[members]]`,
  `[capabilities]`, `[capabilities.limits]`

A decoder MUST verify this by re-serializing the parsed manifest under these
rules and comparing the result byte-for-byte with the manifest as stored,
rejecting a mismatch with `bundle.manifest_not_canonical`.

That verification is only possible because §7 rejects unknown keys everywhere.
An ignored key is a key the decoder cannot re-emit, so a decoder that tolerated
one could not reproduce the bytes it was given, and every extended manifest
would fail this check. The two rules stand or fall together: this format keeps
both and gives up the ability to add an ignorable field without a version bump.

The consequence worth stating plainly: **one logical manifest has exactly one
byte representation, and anything else is not a bundle.** Two producers building
the same application from the same inputs cannot produce two different digests.

## 6. Runtime validation

`runtime` declares which backend a bundle targets. Validation of that claim is
deliberately **asymmetric**, because the two artifact kinds are not equally
self-describing.

### 6.1 `wasm` — byte preamble check

The entrypoint member MUST begin with the eight-byte component-model preamble:

```
00 61 73 6D 0D 00 01 00
```

That is the `\0asm` magic, then a 16-bit little-endian version `0x000D`, then a
16-bit little-endian layer `0x0001`. Layer `1` is what distinguishes a
**component** from a core module.

A core WebAssembly module begins `00 61 73 6D 01 00 00 00` — magic, then a
32-bit version of 1, which reads as version `0x0001` and layer `0x0000`. Core
modules are **rejected at ingest** with `bundle.wasm_not_component`. #16 requires
components; accepting a core module here only defers the failure to a point
where the error is less clear and the artifact has already been stored.

An entrypoint shorter than eight bytes, or not beginning with `\0asm`, or
beginning with `\0asm` and any other version/layer pair, is
`bundle.wasm_preamble_invalid`.

Nothing beyond the preamble is examined. Validating the section structure is the
runtime's job, not the format's.

### 6.2 `microvm` — structural only

No content check is performed, and none is possible: a root filesystem image has
no header announcing what it is. Stating a symmetry here that the code cannot
deliver would be worse than stating the asymmetry.

A `microvm` bundle is validated structurally and no further: the manifest is
well-formed, the required members are present, and `app.entrypoint` names a
member that exists in the archive. Whether that member is a bootable rootfs, and
whether the agent inside it will start, is determined when the bundle is run.

## 7. Forward compatibility

**7.1 An unknown key is rejected, wherever it appears.** Top level, `[app]`,
inside a `[[members]]` element, `[capabilities]`, `[capabilities.limits]` — a
key this version does not define is a key it cannot evaluate, and the bundle is
rejected rather than partly understood.

```toml
schema_version = 1
runtime = "wasm"
telemetry = true             # -> bundle.unknown_key
```

```toml
[app]
entrypoint = "app.wasm"
preload = ["warmup.dat"]     # -> bundle.unknown_key
```

**7.2 An unknown key under `[capabilities]` is rejected with its own code.**

```toml
[capabilities]
version = 1
gpu = true                   # -> bundle.capability_unknown_key
```

```toml
[capabilities.limits]
cpu_ms = 5000
memory_mb = 128
egress_bandwidth_mbps = 100  # -> bundle.capability_unknown_key
```

`capabilities.version` is checked before any other key in the subtree; a value
other than `1` is `bundle.capabilities_version_unsupported`. Its role is
narrower than it would be under a lenient rule — it no longer gates whether
unknown keys are rejected, since 7.1 rejects them everywhere. It remains so the
capability vocabulary can be versioned on its own cadence: a runtime can refuse
a capability set it is too old to enforce, and say so by version, without
`schema_version` having to move every time a permission is added.

The outcome is the same rejection; the distinct code carries a materially
different remediation. `bundle.unknown_key` says a producer emitted a field this
version does not define — usually a typo or a newer toolchain. Under
`[capabilities]` it says something more specific: *this bundle is asking for a
permission or a limit that your runtime cannot enforce.* An operator reading a
log needs to tell those apart, so the format tells them apart.

**7.3 Evolution goes through a version field, never through an ignored key.**
Adding, removing or changing the meaning of any key requires bumping
`schema_version`; changing the capability vocabulary alone may instead bump
`capabilities.version`. A v1 decoder rejects a v2 bundle on the version field,
naming the version it does not support, rather than on whichever unknown key it
happened to reach first.

### 7.4 Why unknown keys are not ignored

The lenient alternative — ignore unknown keys in sub-tables, reject them only
under `[capabilities]` — buys one thing: a v1 host can run a v2 bundle that
added a field, without a version bump. This format gives that up, for three
reasons.

**The manifest is identity-bearing.** Its bytes are inside the bundle digest, so
a v2 bundle with an extra field is already a different bundle with a different
digest. "Ignorable" overstates what leniency achieves: it does not make the two
artifacts the same, it only makes the older host willing to run the newer one.

**Ignoring keys and verifying canonical form are mutually exclusive**, and
canonical form is worth more. A decoder that ignores a key cannot re-emit the
manifest it was handed, so it cannot check that the manifest is in canonical
form, so two producers can emit byte-different manifests for identical content
and mint two digests for one artifact. §5.4 explains what that costs. Rejecting
unknown keys buys back the guarantee that one logical manifest has exactly one
byte representation.

**Today's non-security-bearing table is tomorrow's security-bearing one.**
`[capabilities]` is special-cased under the lenient rule because it is *known
now* to carry permissions. Nothing says `[app]` will not later carry `env`,
`run_as` or `mount` — and on the day it does, every deployed v1 host silently
ignores it. The lenient rule requires predicting which future field is dangerous.
The strict rule does not have to be right about that.

What is given up is real: a mixed-version fleet cannot roll a manifest change
forward without every host understanding it. For an identity-bearing document
that gates capabilities, being unable to add a field quietly is closer to a
feature than a cost.

## 8. Validation order

A decoder MUST perform these checks in this order and MUST stop at the first
failure. Order is normative: a bundle that is both malformed and tampered with
reports the same error on every implementation, so error codes are comparable
across the seam and across languages.

| # | Check | Failure |
| --- | --- | --- |
| 0 | If the caller supplied an expected `BundleRef`, compute the bundle digest over the whole stream and compare | `bundle.digest_mismatch` |
| 1 | Length is a non-zero multiple of 512 | `bundle.not_tar` |
| 2 | Walk blocks: header fields per §3.4 — magic, version, typeflag, prefix, mode, uid, gid, mtime, uname, gname, linkname, devmajor, devminor | `bundle.magic_invalid`, `bundle.entry_type_invalid`, `bundle.prefix_not_empty`, `bundle.mode_invalid`, `bundle.metadata_not_canonical` |
| 3 | Per header: `size` encoding and ceiling | `bundle.size_encoding_invalid`, `bundle.member_too_large` |
| 4 | Per header: checksum recomputed and re-encoded | `bundle.checksum_invalid` |
| 5 | Per entry: path rules of §3.2 | `bundle.path_invalid`, `bundle.path_not_nfc`, `bundle.path_too_long`, `bundle.path_duplicate` |
| 6 | Per entry: data padding is NUL | `bundle.padding_invalid` |
| 7 | Entry order per §3.3 | `bundle.entries_not_sorted` |
| 8 | Trailer is exactly two zero blocks with nothing after | `bundle.trailer_invalid` |
| 9 | `clwd.toml` present | `bundle.manifest_missing` |
| 10 | Manifest parses as TOML | `bundle.manifest_not_toml` |
| 11 | `schema_version` present and equals 1 | `bundle.schema_version_missing`, `bundle.schema_version_unsupported` |
| 12 | No unknown key at the top level or in any table outside `[capabilities]` | `bundle.unknown_key` |
| 13 | `capabilities.version` equals 1 | `bundle.field_missing`, `bundle.capabilities_version_unsupported` |
| 14 | No unknown key anywhere under `[capabilities]` | `bundle.capability_unknown_key` |
| 15 | Required keys present, types correct, values in range, arrays sorted and duplicate-free | `bundle.field_missing`, `bundle.field_type_invalid`, `bundle.field_value_invalid`, `bundle.runtime_unknown` |
| 16 | Manifest re-serializes byte-identically per §5.4 | `bundle.manifest_not_canonical` |
| 17 | Every member digest string is well-formed | `bundle.digest_malformed` |
| 18 | `clwd.toml` not self-listed; listed set equals archive set minus the manifest; `[[members]]` sorted | `bundle.manifest_self_listed`, `bundle.member_missing`, `bundle.member_unlisted`, `bundle.field_value_invalid` |
| 19 | Each member's actual length equals its declared `size` | `bundle.member_size_mismatch` |
| 20 | Each member's BLAKE3 equals its declared `digest` | `bundle.member_digest_mismatch` |
| 21 | `app.entrypoint` names a listed member | `bundle.entrypoint_not_a_member` |
| 22 | Runtime check per §6 | `bundle.wasm_not_component`, `bundle.wasm_preamble_invalid` |

Step 16 comes after step 15 because re-serialization needs a fully validated
model: canonical form is defined over values of the right type in the right
range, so checking it against a manifest that has not been through step 15 would
report a formatting fault for what is really a type error.

### 8.1 Why member digests are verified before the runtime preamble check

Step 20 precedes step 22 deliberately.

The preamble check *interprets* the entrypoint's bytes — it reads them and
branches on what they say. The member digest check *authenticates* them. Running
the interpretation first would mean deciding, on unauthenticated input, that a
bundle is a valid component; a tampered entrypoint could present a correct
preamble in front of arbitrary content and be reported as a component-model
problem, or as no problem at all, when the real fault is that the archive has
been modified.

The general rule the order encodes: **structure before content, and
authentication before interpretation.** Cheap structural checks that need no
hashing come first, so a truncated or non-canonical archive fails without the
decoder hashing megabytes to reach the same conclusion. Then digests. Only then
does anything look at what a member's bytes mean.

The bundle digest is step 0 rather than last for the same reason: when the caller
already knows which bundle it asked for, there is no reason to parse untrusted
structure before confirming this is that bundle.

## 9. Rejection catalogue

Every rejectable condition, with its stable error code. Codes are in the
`bundle.*` namespace and carry the envelope shape established for the CLI:
`{code, message, hint, retryable}`.

**Every code here is `retryable: false`.** A malformed bundle does not become
well-formed on a second attempt; retrying is always the wrong response.

Renaming a code is a breaking change: these cross the repository seam, and the
platform builder reports them to users who did not build this workspace from
source.

### Container

| Code | Condition |
| --- | --- |
| `bundle.not_tar` | Length is zero or not a multiple of 512, or the stream ends mid-entry |
| `bundle.magic_invalid` | `magic`/`version` are not `ustar\0` + `00` |
| `bundle.entry_type_invalid` | `typeflag` is not `0x30` — covers directories, symlinks, hard links, devices, FIFOs, and PAX/GNU extension entries |
| `bundle.prefix_not_empty` | The 155-byte `prefix` field is not all NUL |
| `bundle.mode_invalid` | `mode` is neither `0000644\0` nor `0000755\0` |
| `bundle.metadata_not_canonical` | `mtime`, `uid` or `gid` non-zero, or `uname`, `gname`, `linkname`, `devmajor` or `devminor` non-empty |
| `bundle.checksum_invalid` | Header checksum does not recompute, or is not encoded as six octal digits, NUL, space |
| `bundle.size_encoding_invalid` | `size` is not 11 octal digits followed by NUL — includes GNU base-256 |
| `bundle.member_too_large` | A declared size is 8589934592 or greater |
| `bundle.padding_invalid` | A data padding byte is not NUL |
| `bundle.entries_not_sorted` | Entries are not byte-wise ascending by path |
| `bundle.trailer_invalid` | Trailer is not exactly two zero blocks, or bytes follow it |

### Paths

| Code | Condition |
| --- | --- |
| `bundle.path_invalid` | Not UTF-8, empty, absolute, trailing slash, empty segment, `.` or `..` segment, or a control character |
| `bundle.path_not_nfc` | Not in Unicode Normalization Form C |
| `bundle.path_too_long` | Longer than 100 bytes when UTF-8 encoded |
| `bundle.path_duplicate` | Two entries share a path, or two source paths normalize to one |

### Manifest

| Code | Condition |
| --- | --- |
| `bundle.manifest_missing` | No `clwd.toml` entry |
| `bundle.manifest_not_toml` | `clwd.toml` is not valid TOML |
| `bundle.schema_version_missing` | No `schema_version` key |
| `bundle.schema_version_unsupported` | `schema_version` is not `1` |
| `bundle.unknown_key` | An unrecognized key at the top level or in any table outside `[capabilities]` |
| `bundle.manifest_not_canonical` | The manifest does not re-serialize byte-identically under §5.4 |
| `bundle.capability_unknown_key` | An unrecognized key anywhere under `[capabilities]` |
| `bundle.capabilities_version_unsupported` | `capabilities.version` is not `1` |
| `bundle.field_missing` | A required key is absent |
| `bundle.field_type_invalid` | A key has the wrong TOML type |
| `bundle.field_value_invalid` | Right type, disallowed value: out of range, empty string, duplicate or unsorted array element, unsorted `[[members]]` |
| `bundle.runtime_unknown` | `runtime` is neither `"wasm"` nor `"microvm"` |

### Members

| Code | Condition |
| --- | --- |
| `bundle.manifest_self_listed` | `clwd.toml` appears in `[[members]]` |
| `bundle.member_missing` | A listed member is not in the archive |
| `bundle.member_unlisted` | An archive entry other than `clwd.toml` is not listed |
| `bundle.member_size_mismatch` | Declared `size` differs from the entry's actual length |
| `bundle.member_digest_mismatch` | Declared `digest` differs from the entry's BLAKE3 |
| `bundle.digest_malformed` | A digest or `BundleRef` string is not `blake3:` plus 64 lowercase hex characters |

### Entrypoint and runtime

| Code | Condition |
| --- | --- |
| `bundle.entrypoint_not_a_member` | `app.entrypoint` does not name a listed member |
| `bundle.wasm_not_component` | `runtime = "wasm"` and the entrypoint is a core module |
| `bundle.wasm_preamble_invalid` | `runtime = "wasm"` and the entrypoint is not WebAssembly at all |

### Bundle

| Code | Condition |
| --- | --- |
| `bundle.digest_mismatch` | Computed bundle digest differs from the expected `BundleRef` |

## 10. Conformance vector

An implementation that produces these bytes for these inputs is canonicalizing
correctly. An implementation that does not has a bug, wherever it is.

This vector is the arbiter, not the prose above it. A specification paragraph
can be misread; a byte array cannot. An implementation SHOULD commit these bytes
as a test fixture and assert its encoder reproduces them exactly, and an
independent implementer SHOULD check against them before anything else in this
document.

**Input:** two files, no executable bit.

`app.wasm` — 8 bytes, the component-model preamble, which is also the smallest
valid component:

```
00 61 73 6D 0D 00 01 00
```

```
size   = 8
digest = blake3:48d3a2e8a9b0cde6293ba7f68824a97d069d096f44335a9e86aee55238dcdcb8
```

`clwd.toml` — 320 bytes, exactly:

```toml
schema_version = 1
runtime = "wasm"

[app]
entrypoint = "app.wasm"

[[members]]
path = "app.wasm"
digest = "blake3:48d3a2e8a9b0cde6293ba7f68824a97d069d096f44335a9e86aee55238dcdcb8"
size = 8

[capabilities]
version = 1
egress = []
secrets = []
storage_namespaces = []

[capabilities.limits]
cpu_ms = 5000
memory_mb = 128
```

```toml
size   = 320
digest = blake3:7e775181bd6658c8579ae0970bc214b43347f17b6b27e9eb817a088f8126a849
```

**Output:** 3072 bytes, six blocks. `app.wasm` sorts before `clwd.toml` because
`0x61` < `0x63`.

| Offset | Contents |
| --- | --- |
| 0 | header, `app.wasm`, `size` = `00000000010\0` (octal 10 = 8), `chksum` = `30 30 37 34 30 35 00 20` |
| 512 | 8 content bytes, 504 NUL padding |
| 1024 | header, `clwd.toml`, `size` = `00000000500\0` (octal 500 = 320), `chksum` = `30 30 37 35 36 36 00 20` |
| 1536 | 320 content bytes, 192 NUL padding |
| 2048 | 512 NUL — first trailer block |
| 2560 | 512 NUL — second trailer block |

```
bundle digest = blake3:5fc9042ff861616e77ec7b6b37a70a806e19c94d70b4c8fe016f52f30d7209a7
```

The digest covers all 3072 bytes, trailer included.

## 11. Outside this specification

Named so the boundaries are not inferred:

- **Compression at rest.** §2.1. The platform's storage layer.
- **What capability strings mean, and their enforcement.** §5.3. #33.
- **How a bundle is produced from a source project** — templates, toolchains,
  which files become members. #28. This document defines the format, not the
  pipeline that fills it.
- **Whether a `microvm` entrypoint is a bootable image.** §6.2. Determined at
  run time.
- **Signing and provenance.** No signature envelope in v1. A signature over the
  bundle digest needs no format change; one inside the archive would.
- **Snapshot artifacts.** #35. A snapshot is not a bundle.
- **Registry, naming, versioning.** A bundle is addressed by digest. Human names
  live in metadata about bundles.
