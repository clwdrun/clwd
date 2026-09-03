//! The app's key-value store, and the per-caller scoping it does not do for you.
//!
//! # The thing most authors get wrong
//!
//! **This namespace belongs to the app, not to the caller.** Every caller of
//! one app reads and writes the same keys: caller A stores a note and caller B
//! reads it. That is the feature, not a leak — the data belongs to the
//! application the way rows in its database do.
//!
//! It is also the opposite of what most people assume, because `clwd:identity`
//! sits in the same world and *is* per-caller. The WIT spends a paragraph on
//! this (DECISION 1 in `wit/deps/app-data/app-data.wit`) and it is worth
//! repeating: an app that wants per-caller data must scope its own keys.
//!
//! # Two functions, and neither one hides the choice
//!
//! [`shared`] is the app-wide namespace. [`for_caller`] is one scoped to a
//! particular caller. Both are named at the call site, so which one an app
//! meant is visible in the diff:
//!
//! ```ignore
//! data::shared().set("config", b"...")?;          // everyone sees this
//! data::for_caller(&caller)?.set("note", b"...")?; // only this caller does
//! ```
//!
//! There is deliberately **no scoped-by-default** convenience. Making the safe
//! case invisible would undo the thing the WIT went out of its way to surface —
//! and an author who never learns the namespace is shared will be surprised by
//! it somewhere less convenient than here.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::caller::Caller;
use crate::clwd::app_data::store;

/// Why a store operation could not be performed.
///
/// Note what is not here: "key not found". Absence is an ordinary result, and
/// [`Namespace::get`] returns `Ok(None)` for it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DataError {
    /// The key is not a legal key. The string says why.
    #[error("invalid key: {0}")]
    InvalidKey(String),
    /// The value is larger than the store accepts. The number is the limit.
    #[error("value too large; the limit is {0} bytes")]
    ValueTooLarge(u64),
    /// The store is not answering.
    #[error("the store is unavailable")]
    Unavailable,
}

impl From<store::Error> for DataError {
    fn from(e: store::Error) -> Self {
        match e {
            store::Error::InvalidKey(why) => Self::InvalidKey(why),
            store::Error::ValueTooLarge(limit) => Self::ValueTooLarge(limit),
            store::Error::Unavailable => Self::Unavailable,
        }
    }
}

impl From<DataError> for crate::Error {
    /// A store failure as something to tell the caller.
    ///
    /// All three become [`Error::Internal`](crate::Error::Internal): every one
    /// is the app's own problem — a key it chose, a value it sized, a backend
    /// it depends on — and none is something the caller can act on. The detail
    /// stays in the log where the author can see it.
    fn from(_: DataError) -> Self {
        Self::Internal
    }
}

/// A key-value namespace.
///
/// Obtained from [`shared`] or [`for_caller`]; the difference between them is
/// the prefix this puts on every key.
#[non_exhaustive]
pub struct Namespace {
    prefix: Option<String>,
}

impl Namespace {
    fn key(&self, key: &str) -> String {
        match &self.prefix {
            Some(p) => format!("{p}/{key}"),
            None => String::from(key),
        }
    }

    /// The value of `key`, or `None` if nothing is stored under it.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, DataError> {
        store::get(&self.key(key)).map_err(DataError::from)
    }

    /// Store `value` under `key`, replacing whatever was there.
    ///
    /// An empty value is legal and is not the same as absence.
    pub fn set(&self, key: &str, value: &[u8]) -> Result<(), DataError> {
        store::set(&self.key(key), value).map_err(DataError::from)
    }

    /// Remove `key`. Removing what is not there succeeds.
    pub fn delete(&self, key: &str) -> Result<(), DataError> {
        store::delete(&self.key(key)).map_err(DataError::from)
    }
}

/// The app-wide namespace: every caller sees the same keys.
#[must_use]
pub fn shared() -> Namespace {
    Namespace { prefix: None }
}

/// A namespace private to one caller.
///
/// Keys are prefixed with the caller's subject, so two callers using the same
/// key do not see each other's values.
///
/// Returns `None` for [`Caller::Anonymous`], because an anonymous caller has no
/// subject to scope by and there is no sensible namespace to invent. That is
/// the same reasoning as everywhere else in this crate: anonymous is a kind of
/// caller, and the type says what it cannot do rather than quietly picking a
/// key like `""` that every anonymous caller would then share.
#[must_use]
pub fn for_caller(caller: &Caller) -> Option<Namespace> {
    caller.subject().map(|subject| Namespace {
        // `u/` rather than the bare subject, so a per-caller key can never
        // collide with an app-wide key an author chose.
        prefix: Some(format!("u/{subject}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::{Service, User};

    fn user(subject: &str) -> Caller {
        Caller::User(User {
            subject: String::from(subject),
            org: String::from("acme"),
            display_name: None,
            email: None,
        })
    }

    #[test]
    fn the_shared_namespace_does_not_rewrite_keys() {
        assert_eq!(shared().key("config"), "config");
    }

    /// The scoping is what stops two callers colliding. If this ever returns
    /// the bare key, every app using `for_caller` silently becomes shared.
    #[test]
    fn a_caller_namespace_prefixes_by_subject() {
        let ns = for_caller(&user("alice")).expect("a user has a subject");
        assert_eq!(ns.key("note"), "u/alice/note");
    }

    #[test]
    fn two_callers_do_not_collide() {
        let a = for_caller(&user("alice")).expect("subject");
        let b = for_caller(&user("bob")).expect("subject");
        assert_ne!(a.key("note"), b.key("note"));
    }

    /// A per-caller key must not be able to land on an app-wide one, however
    /// the author names it.
    #[test]
    fn a_caller_key_cannot_collide_with_an_app_wide_key() {
        let ns = for_caller(&user("alice")).expect("subject");
        assert_ne!(ns.key("config"), shared().key("config"));
        assert!(ns.key("config").starts_with("u/"));
    }

    /// Anonymous has no namespace, and the type says so rather than inventing
    /// one that every anonymous caller would share.
    #[test]
    fn an_anonymous_caller_has_no_private_namespace() {
        assert!(for_caller(&Caller::Anonymous).is_none());
    }

    #[test]
    fn a_service_caller_gets_one_keyed_by_its_own_subject() {
        let c = Caller::Service(Service {
            subject: String::from("billing"),
            org: String::from("acme"),
            on_behalf_of: None,
        });
        let ns = for_caller(&c).expect("a service has a subject");
        assert_eq!(ns.key("cursor"), "u/billing/cursor");
    }

    /// Every store failure is the app's own problem, not the caller's.
    #[test]
    fn a_store_failure_is_reported_as_internal_not_as_the_callers_fault() {
        let e: crate::Error = DataError::Unavailable.into();
        assert_eq!(e.status(), 500);
    }
}
