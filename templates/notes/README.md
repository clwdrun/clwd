# notes

Each person writes notes, sees only their own, and deletes them.

## Run it

```
clwd dev
```

Then open `http://localhost:8787/`. When it works, deploy it:

```
clwd deploy
```

**Deployed, this app cannot store notes yet**: hosted app-data is not
available to deployed apps (clwdrun/clwd#91), so every write answers `500`.
Under `clwd dev` it works, with an in-memory store that is emptied whenever the
app is rebuilt.

## What it does

| Method | Path | Response |
| --- | --- | --- |
| GET | `/` | `200` HTML: a form, and the caller's notes, newest first |
| POST | `/notes` | stores the form's `text`, then `303` to `/`; `400` if empty or over 2000 bytes |
| POST | `/notes/<id>/delete` | deletes that note, then `303` to `/` |
| any | anything else | `404` |

## Invariants

- **Storage belongs to the app, not to the caller.** `data::for_caller` scopes
  keys to the caller's subject; `data::shared()` does not. Replacing one with
  the other compiles and shows every person everyone's notes.
- **The subject is pairwise**: the same person has a different subject in every
  app. Key this app's data by it; never send it elsewhere or compare it with a
  subject from another app.
- **No state change on `GET`.** Creating and deleting are `POST`. A browser
  follows a `GET` link from any other site, with the app's cookies, so a `GET`
  that deletes is a link that deletes.
- **Identity comes only from `caller()`.** Never from a form field, query
  parameter, header or cookie. There is no authentication code in this app.
- **Every value that reaches HTML goes through `Html::text`.** A note containing
  `<script>` is shown as text. Never put a stored value in `href`, `src`,
  `<script>` or an `on*` attribute: escaping does not cover those.
- `data::for_caller` returns `None` for two different reasons: the caller is
  anonymous (`401`), or their subject cannot appear in a storage key (`500`,
  logged, and yours to fix).
- Detail goes to `println!`, which `clwd logs` shows and the caller never sees.
  Never put a subject or a storage error in a response.
- `clwd:app-data` has no key listing and no atomic update. The app keeps its own
  index of note ids, and two submissions from the same person at the same
  instant can drop one note from it.

## Who is calling

What each field of `caller()` guarantees when the app is deployed:

- **`subject`**: opaque, stable for this person in this app, pairwise.
- **`email`**: a verified address, or `None`.
- **`org`**: the organisation that owns this app.
- **`display_name`**: `None` for now.

There are no roles and no groups. Locally the caller is always subject `local`.

## Works when deployed

Not yet: it waits on hosted app-data (clwdrun/clwd#91). Use `hello` to try a
deploy today.
