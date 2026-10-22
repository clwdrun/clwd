# hello

A page that says hello to whoever is calling, by their verified email.

## Run it

```
clwd dev
```

Then open `http://localhost:8787/`. When it works, deploy it:

```
clwd deploy
```

Locally the caller is always `local@localhost`. Deployed, it is whoever signed
in. The code does not change between the two, only what `caller()` returns.

## What it does

| Method | Path | Response |
| --- | --- | --- |
| GET | `/` | `200` HTML: `hello, <email>` |
| GET | `/health` | `204` |
| any | anything else | `404` |

## Invariants

- **No authentication code.** No login page, session, cookie or token. The host
  decides who is calling before the app runs; the app asks `caller()`.
- **Identity comes only from `caller()`.** Never from a form field, query
  parameter, header or cookie: those are whatever the client chose to send.
- **Every value that reaches HTML goes through `Html::text`.** `Html::raw` takes
  only string literals, so a value cannot reach the page unescaped. Escaping
  covers element text and quoted attributes, not `href`, `src`, `<script>` or
  `on*` attributes: never put a value there.
- **Send pages with `Response::html`.** It sets `Content-Type` and
  `X-Content-Type-Options: nosniff`.
- `email` is optional. Handle `None`; never show the subject instead — it is an
  opaque id, not something to show people.
- Do not use `.unwrap()`. A panic in a guest is a trap that ends the instance.
  Return an `Error` instead.
- Keep `[profile.release]` exactly as it is. It is the largest single lever on
  how big this app is, and `clwd build` warns if a key goes missing.

## Who is calling

What each field of `caller()` guarantees when the app is deployed:

- **`subject`**: opaque and stable for this person in this app. It is
  **pairwise**: the same person has a different subject in every other app, so
  never compare or share subjects between apps.
- **`email`**: a verified address, or `None`.
- **`org`**: the organisation that owns this app.
- **`display_name`**: `None` for now.

There are no roles and no groups.

## Works when deployed

Yes. It uses no storage, no secrets and no outbound network access.

## Deliberately absent

- Storage: see `notes`.
