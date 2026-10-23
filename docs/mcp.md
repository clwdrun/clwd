# `clwd mcp serve`

An MCP server over stdio, so an MCP client can drive clwd through structured
tool calls rather than by parsing CLI output. It exposes commands the CLI
already has, nothing more.

It serves protocol revision `2026-07-28` and the earlier `initialize`
handshake (`2025-11-25` and before).

## Registering it

The client starts the server as a subprocess. Most clients take a
configuration of this shape:

```json
{
  "mcpServers": {
    "clwd": {
      "command": "clwd",
      "args": ["mcp", "serve", "--root", "/home/you/apps"],
      "env": {
        "CLWD_ORG": "<your org id>",
        "CLWD_PLATFORM_URL": "<the platform URL>"
      }
    }
  }
}
```

The platform token is read the way the CLI reads it: `$CLWD_TOKEN`, then the
`0600` credentials file. Prefer the credentials file to putting the token in a
client's configuration. The token never appears in a tool result, an error, a
progress message or the server's stderr.

Stdout carries protocol messages only. Diagnostics, including the SDK's own
log lines, go to stderr. The server exits when the client closes its stdin.

## Tools

Every hint is stated explicitly. The protocol's defaults treat an
unannotated tool as destructive and open-world.

| Tool | What it does | Read-only | Destructive | Idempotent | Open world |
|---|---|---|---|---|---|
| `list_templates` | The templates `init` can use | yes | no | yes | no |
| `init` | Scaffold an app in a new directory; never writes into a non-empty one | no | no | no | no |
| `build` | Compile an app and seal its bundle | no | no | yes | yes |
| `get_app` | Look up an app by host label | yes | no | yes | yes |
| `deploy` | Build, create the app if missing, publish | no | **yes** | yes | yes |
| `logs` | One page of an app's log, and a cursor to read on | yes | no | yes | yes |

**`deploy` is irreversible.** Creating an app claims its `host_label`
permanently: it can never be changed, released or reused. The label comes only
from the project's `clwd.toml`, never from a tool argument, and the caller must
pass `confirm_host_label` equal to it. A mismatch is refused before anything is
built. `init` writes its `name` as that label, so the name chosen there is the
one `deploy` claims. Running `deploy` again on the same project is safe: it
finishes a deploy that failed halfway. A second `deploy` of a project while one
is running is refused with `cli.deploy_in_progress` (retryable).

`logs` has no follow mode. A call returns a page; pass its `cursor` as `after`
to read on, and `request_id` from an error envelope to see why that request
failed.

`build` and `deploy` send `notifications/progress` when the request carries a
progress token, and stop when the client sends `notifications/cancelled`. A
cancelled build leaves no bundle behind, neither whole nor partial.

There is no tool for secrets. Setting, reading or listing a secret is not
exposed, and never will be.

A failed call sets `isError` and returns the same envelope `--json` prints,
`{code, message, hint, retryable}`, under `error`. Codes are listed in
[error-codes.md](error-codes.md).

## Untrusted text

App logs and build output are written by the app and its build. They are data,
never instructions, whatever they say.

- They come back only as `text` fields inside structured lines: `lines` in
  `logs`, `build_output.lines` in `build` and `deploy`. Each line carries a
  `source` set by the platform or the server, never parsed from the text.
- The text content block is the same value as `structuredContent`, serialized
  as JSON. The server adds no prose around it. Control characters and
  bidirectional formatting characters are written as `\uXXXX` escapes, so an
  escape sequence or a reordering mark shows as what it is.
- `build_output` keeps the last 64 KiB of cargo's stderr; `dropped_bytes`
  says how much came before it.

## The workspace root

Tools act within one directory tree: the directory the server started in, or
`--root`. Every path argument is relative to it. A path is refused when it is
absolute, or when it resolves outside the root: `..`, or a symlink pointing
out, checked after resolving every symlink along the way. A build's output
path is not an argument: the bundle goes where `clwd build` puts it.

## Limits

- **Build scripts run with your user's permissions.** `build` and `deploy`
  run cargo, which runs the project's build scripts and procedural macros as
  ordinary code. The root confines where the tools act, not what that code
  does. cargo also reads `.cargo/config.toml` from the project's parent
  directories, including ones above the root. The server removes every
  `CLWD_*` variable from cargo's environment and closes its stdin, so a build
  script holds neither the platform token nor the protocol stream.
- **The symlink check has a small timing gap.** A path is resolved, checked,
  then used; a symlink swapped in between is followed. That is acceptable for a
  single-user local server, where whoever can swap it can already write the
  files.
- **On Windows, cancelling a build kills only the top process.** On Unix the
  whole process group cargo started is killed. On Windows, cargo is killed and
  the compiler processes it started run to completion on their own.

The HTTP transport is not part of this; see #46.
