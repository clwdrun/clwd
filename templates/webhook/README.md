# webhook

Receives a signed webhook, verifies it, acknowledges it.

## Run it

```
clwd secrets set webhook_signing_key < key.txt
clwd build .
clwd run app.clwd --path /
```

`clwd run` sends `GET`, which this app answers `404`. That is correct: it serves
`POST` only. Sending a real `POST` needs a deployed app.

## What it does

| Method | Path | Response |
| --- | --- | --- |
| POST | any | `204` if the signature verifies, `403` if not, `400` if the body or signature header is missing |
| any other | any | `404` |

## Invariants

- **`signature_matches` is a placeholder and is not secure.** Replace it with an
  HMAC over the body, keyed with the secret, compared in constant time. It is
  left out because GitHub, Stripe and Slack each specify a different
  construction, and a plausible wrong one is worse than an obvious stub.
- Read headers before the body. `into_body()` consumes the request.
- `secret()` returns `None` for "not granted" and for "does not exist", and
  those are deliberately the same answer. Do not build anything that tries to
  tell them apart.
- A missing secret is your app misconfigured, not the caller's problem: log
  which name is missing, return `500`, and never name it in the response.
- A bad signature is `403`, not `400`. The request was well formed and is not
  allowed; `400` sends the sender looking for a typo.

## Deliberately absent

- Storage — see `notes`.
- Replay protection. A real webhook receiver records message ids it has already
  processed. This one does not, and that is a gap you must close.
- Outbound requests. `egress` is empty; add a host to `clwd.toml` to make one.

## Local versus deployed — `platform#11`

This app does not read identity, so nothing here changes when `platform#11`
lands. The secret it reads is real locally: `clwd secrets set` stores it, and
`clwd run` injects it exactly as the platform will.
