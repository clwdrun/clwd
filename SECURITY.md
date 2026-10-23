# Security policy

clwd runs code its authors did not write, so the isolation boundary is the
product. This repository is public so that boundary can be audited.

- [Threat model](docs/security/threat-model.md): what clwd protects, from
  whom, and what it does not yet do.
- [Invariants](docs/security/invariants.md): each guarantee, and the test
  that enforces it.

## Reporting a vulnerability

**Report privately. Do not open a public issue, pull request or discussion.**

Use GitHub private vulnerability reporting:
<https://github.com/clwdrun/clwd/security/advisories/new>

This is the only channel. Use it for the hosted service too; reports about it
are routed internally.

A useful report includes:

- what breaks: which invariant or boundary, and which crate or interface the
  path starts from;
- a reproduction: a component, a bundle, a request sequence, a command, or a
  `cargo test` case;
- the commit or tag you tested.

A report does not need a full exploit chain. Breaking an invariant in
[invariants.md](docs/security/invariants.md) is a finding on its own.

## What happens next

| Stage | Target |
|---|---|
| Acknowledgement | 3 business days |
| Triage: confirmed or not, with a severity | 10 business days |
| Fix released, critical severity | 30 days from the report |
| Fix released, other severities | 90 days from the report |

We keep you informed at each stage, agree the disclosure date with you, and
credit you in the advisory unless you ask us not to. There is no paid bounty.

## Disclosure

Disclosure is coordinated. The default embargo is 90 days from the report, or
until a fix is released if that comes first. We may agree a different date
with you, for example to shorten it when a fix ships early, or to extend it
when a fix depends on an upstream project.

Each fixed vulnerability is published as a GitHub Security Advisory on this
repository, with a CVE requested through GitHub. Once clwd's crates are
published on crates.io, advisories for them will also go to the RustSec
advisory database; none is published yet.

## Supported versions

clwd is pre-1.0. Supported: `main` and the latest `v0.0.x` tag. A fix lands
on `main` and ships in the next tag; earlier tags get no backports.

## Scope

In scope: the code in this repository, including

- escaping the runtime sandbox, or reaching anything the
  [threat model](docs/security/threat-model.md) says a guest cannot;
- one app or caller reading or changing another's data, secrets, logs or
  identity;
- a bundle that verifies but is not what was built;
- the CLI or MCP server leaking the platform token or a secret, acting outside
  its workspace root, or being steered by app output;
- a claim in the threat model or the invariants that the code does not
  uphold.

Out of scope:

- items already listed under
  [Known limitations](docs/security/threat-model.md#known-limitations), unless
  you show them to be worse than described;
- bugs in apps people deploy, unless clwd failed to contain them;
- a vulnerability in a dependency with no path to exploitation through clwd
  (report it upstream, and tell us if we should pin or patch);
- volumetric denial of service against hosted infrastructure.

## Safe harbour

We will not pursue legal action against, or ask a provider to act against,
anyone who researches and reports in good faith under this policy. Good faith
means you:

- access no more data than a proof of concept needs, and delete what you
  obtained once the report is resolved;
- test only against your own apps, accounts and machines, never other
  people's;
- do not degrade the availability of the hosted service or anyone's app;
- give us the time above to fix the issue before disclosing it.

If something you want to test is not clearly covered, ask through the
reporting channel first.
