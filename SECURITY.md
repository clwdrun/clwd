# Security Policy

clwd runs code that its authors did not write, submitted by people who are
not engineers, against real company data. The isolation boundary is the
product. This repository is public specifically so that boundary can be
audited — a disclosure policy is part of that bargain.

## Reporting a vulnerability

**Report privately. Do not open a public issue.**

Use GitHub private vulnerability reporting:
<https://github.com/clwdrun/clwd/security/advisories/new>

That channel is the one to use even if the flaw is in the private control
plane rather than in this workspace — routing is our problem, not yours.

Please include:

- what isolation property breaks, and which crate or WIT interface it starts from
- a reproduction: a component, a request sequence, or a `cargo test` case
- which backend it applies to (`wasm`, `microvm`, or both)
- the commit or released version you tested

What to expect:

| Stage | Target |
| --- | --- |
| Acknowledgement | 3 business days |
| Initial assessment | 10 business days |
| Fix or mitigation plan communicated | 30 days |

We will credit you in the advisory unless you ask us not to. We do not
currently run a paid bounty.

## Supported versions

clwd is pre-1.0. Only the latest commit on `main` and the most recent
tagged release receive security fixes. There are no backports to earlier
0.x tags.

`clwd-sdk` publishes to crates.io on its own cadence; a security fix there
ships as a new patch version, never as a silent re-tag.

## Scope

In scope — these are the properties the sandbox is supposed to hold:

- **Sandbox escape.** Guest code reaching host filesystem, host network, or
  host process state through either backend.
- **Cross-tenant access.** One app or org observing or mutating another's
  data, logs, or secrets.
- **Identity spoofing.** A component forging, mutating, or replaying a
  `clwd:identity` principal, role, or group — including making `anonymous`
  present as an authenticated caller, or a caller's identity leaking across
  request boundaries on a reused instance.
- **Authorization bypass.** `authorize` returning `allowed` where the
  control plane's policy says otherwise, or failing open when the control
  plane is unreachable.
- **Secret exfiltration.** Any path from app code to a secret it was not
  granted.
- **Resource exhaustion that breaks isolation** — a guest starving or
  crashing its host, as distinct from merely exhausting its own quota.

Out of scope:

- Vulnerabilities in apps that users deploy. That code is the user's, not
  ours — unless the flaw is that clwd failed to contain it, which is in
  scope and belongs above.
- Findings in a dependency with no demonstrated exploit path through clwd.
  Report those upstream; tell us if we should pin or patch.
- Volumetric denial of service against hosted infrastructure.
- Missing hardening headers or similar findings on marketing surfaces.

## Boundary assumptions

Two assumptions are load-bearing. If you can break either, that is a
finding even without a full exploit chain:

1. **A component cannot author its own identity.** Everything in
   `clwd:identity` is populated by the gateway before app code runs, and
   there is no constructor exposed to the guest.
2. **Backends are interchangeable.** Anything the `wasm` backend enforces,
   the `microvm` backend must enforce with the same vocabulary. A property
   that holds on one and not the other is a vulnerability in the weaker one,
   not a documentation gap.
