# notes

One note per caller, stored and read back.

## Run it

```
clwd build .
clwd run app.clwd --path /note
```

`clwd run` sends `GET`. `PUT` and `DELETE` are reachable from a deployed app.

## What it does

| Method | Path | Response |
| --- | --- | --- |
| GET | `/note` | `200` with the note, or `404` |
| PUT | `/note` | `204`, body stored |
| DELETE | `/note` | `204` |
| any | anything else | `404` |

## Invariants

- **Storage belongs to the app, not to the caller.** Every caller of this app
  reads and writes the same keys unless you scope them. `data::for_caller`
  scopes them; `data::shared()` does not. Replacing one with the other compiles
  and makes every caller share one note.
- `data::for_caller` returns `None` for two different reasons: the caller is
  anonymous, or their subject cannot appear in a storage key. These are not the
  same failure and must not be reported the same way — the first is `401`, the
  second is `500` and yours to fix.
- A subject is never reshaped to fit the key grammar. Two subjects mangled into
  one key would put two callers in one namespace.
- Detail goes to `println!`, which `clwd logs` shows and the caller never sees.
  Never put a subject id or a stored value in a response body.
- Format with `{}`, not `{:?}`, unless you need `Debug`. Debug-formatting a
  string costs about 8 KiB in a component.

## Deliberately absent

- Secrets and request-body verification — see `webhook`.
- Listing keys. `clwd:app-data` has no `list-keys`, on purpose.
- Any atomic read-modify-write. There is none, and this app does not pretend
  otherwise.

## Local versus deployed — `platform#11`

`caller()` returns a verified caller. This app never sees a token or a session,
and there is no authentication code in it. That is true today.

**The value is not real yet.** Locally the caller is always subject `local` in
org `local`. Real subject ids arrive with `platform#11`. The code below does not
change when they do — only what it receives.
