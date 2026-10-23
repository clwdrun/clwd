//! A tool that panics, under `clwd mcp serve`: the panic goes to stderr, the
//! client gets a JSON-RPC error for that request, stdout stays protocol only,
//! and the server keeps serving.
//!
//! Needs the binary built with `mcp-test-panic`, so `Cargo.toml` names that as
//! this target's required feature and `just mcp-panic` runs it.
//! `just mcp-release-bare` checks the release binary has no such tool.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// Invariant: MCP-1.
#[test]
fn a_panicking_tool_never_reaches_stdout() {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_clwd"))
        .args(["mcp", "serve", "--root"])
        .arg(env!("CARGO_TARGET_TMPDIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the clwd binary is built as a test dependency");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut next = || {
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read");
        line
    };

    let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "_meta": meta, "name": "clwd_test_panic", "arguments": {} } });
    writeln!(stdin, "{call}").expect("write");
    let first = next();
    let response: Value = serde_json::from_str(&first).expect("a JSON-RPC line");
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["error"]["code"], -32603, "{response}");

    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list",
        "params": { "_meta": meta } });
    writeln!(stdin, "{list}").expect("write");
    let second = next();
    let listed: Value = serde_json::from_str(&second).expect("a JSON-RPC line");
    assert_eq!(listed["id"], 2, "the server carries on after a panic");

    drop(stdin);
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).expect("read");
    assert_eq!(rest, "", "nothing more on stdout");
    let status = child.wait().expect("wait");
    assert!(status.success(), "{status:?}");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut stderr)
        .expect("read");
    assert!(
        stderr.contains("a test tool panicked"),
        "the panic message went to stderr: {stderr}"
    );
}
