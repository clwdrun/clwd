# minimal

An app that answers HTTP requests. Nothing else.

## Run it

```
clwd build .
clwd run app.clwd --path /
```

## What it does

| Method | Path | Response |
| --- | --- | --- |
| GET | `/` | `200 hello` |
| GET | `/health` | `204` |
| any | anything else | `404 not found` |

## Invariants

- Every path returns a response. An app that does not answer leaves the caller
  waiting for a timeout.
- `Response::send` takes the `ResponseOutparam` by value and is the only way to
  answer. There is no way to write a body stream directly, and that is
  deliberate: the underlying order is unforgiving and getting it wrong hangs
  the request.
- Do not use `.unwrap()`. A panic in a guest is a trap that ends the instance.
  Return an `Error` instead.
- Keep `[profile.release]` exactly as it is. It is the largest single lever on
  how big this app is, and `clwd build` warns if a key goes missing.

## Deliberately absent

- Identity — see `notes`.
- Storage — see `notes`.
- Secrets and request bodies — see `webhook`.

## Local versus deployed

Nothing here depends on the difference. `minimal` behaves identically on a
laptop and in production.
