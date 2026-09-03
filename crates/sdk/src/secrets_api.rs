//! The credentials this app was granted.
//!
//! # There is deliberately no `require_secret`
//!
//! A helper that errors when a secret is absent — `require_secret("stripe_key")`
//! — is convenient, and `clwd secrets get` on the CLI does exactly that. **It
//! is not safe here, and the difference is who is asking.**
//!
//! On the CLI the operator owns the store. Telling them a name is not there
//! leaks nothing, because there is no untrusted party in the room.
//!
//! In a guest the app *is* the untrusted party. An error that distinguishes
//! "this secret does not exist" from "you were not granted it" is a probe: call
//! it in a loop over candidate names and you have enumerated the store, which
//! is the capability that removing `list` from this interface was meant to
//! prevent. `clwd:secrets` collapses the two into a single `none` for that
//! reason — not by a rule someone must remember, but because they are the same
//! value and there is no variant to leak the difference.
//!
//! So a guest gets `Option`, and no way to turn absence into a distinguishing
//! error. An app that needs a secret and does not have one should fail with
//! [`Error::Internal`](crate::Error::Internal) — its own misconfiguration,
//! reported as its own problem:
//!
//! ```ignore
//! let Some(key) = clwd_sdk::secret("stripe_key") else {
//!     println!("stripe_key was not granted; check clwd.toml");
//!     return Error::Internal.respond(out);
//! };
//! ```
//!
//! Someone will read this and the CLI's `get` as inconsistent. They are
//! different on purpose: the same question asked across two different trust
//! boundaries has two different right answers.

use alloc::vec::Vec;

/// The value of `name`, or `None` if this app does not have it.
///
/// `None` does not distinguish "no such secret" from "not granted to this app",
/// and must not be treated as evidence either way — see the module docs. An app
/// that branches on the difference is branching on something it was not told.
#[must_use]
pub fn secret(name: &str) -> Option<Vec<u8>> {
    crate::clwd::secrets::broker::get(name)
}
