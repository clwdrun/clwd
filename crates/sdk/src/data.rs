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
    /// The key this namespace actually stores under.
    ///
    /// Joined with `.`, not `/`: the published grammar in
    /// `wit/deps/app-data/app-data.wit` does not permit `/`, so a slash-joined
    /// key is refused by the host on every call. See
    /// `every_generated_key_satisfies_the_published_grammar`, which exists
    /// because that is exactly what shipped.
    fn key(&self, key: &str) -> String {
        match &self.prefix {
            Some(p) => format!("{p}{SEGMENT_SEPARATOR}{key}"),
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

/// The prefix a per-caller namespace puts before every key.
///
/// **This is an on-disk format.** Every app that has ever used [`for_caller`]
/// has its data under keys beginning with this, so changing it orphans all of
/// it — silently, looking like the app forgot, which is the same failure
/// `DigestAsAppId` warns about on the host side. It is not a cosmetic choice.
///
/// `.` rather than `/` because the published key grammar does not permit `/`;
/// see [`key_grammar_permits`].
const CALLER_PREFIX: &str = "u.";

/// What joins a namespace prefix to an app's own key.
///
/// Also an on-disk format, and also constrained by the published grammar: `/`
/// is not in it.
const SEGMENT_SEPARATOR: char = '.';

/// Whether `segment` satisfies the key grammar `clwd:app-data` publishes.
///
/// **The source of truth is `wit/deps/app-data/app-data.wit`**, which states:
/// "A key is 1 to 128 bytes of `a`-`z`, `0`-`9`, `.`, `_` or `-`, and may not
/// begin or end with `.`, `_` or `-`."
///
/// This is a second implementation of that published rule, not of the host's
/// private one. The host enforces it too; a guest that checks first gets a
/// typed refusal instead of a store error it can do nothing about.
fn key_grammar_permits(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 128
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && !segment.starts_with(['.', '_', '-'])
        && !segment.ends_with(['.', '_', '-'])
}

/// A namespace private to one caller.
///
/// Keys are prefixed with the caller's subject, so two callers using the same
/// key do not see each other's values.
///
/// # When this returns `None`
///
/// **The caller is anonymous.** There is no subject to scope by, and inventing
/// one would give every anonymous caller the same namespace — the leak this
/// exists to prevent.
///
/// **The subject cannot appear in a key.** The grammar in
/// [`key_grammar_permits`] is narrow, and a subject outside it is refused
/// rather than mangled into something that fits. Mangling is the dangerous
/// option: two different subjects can squash into one key, and two callers
/// silently sharing a namespace is a data leak rather than a formatting
/// problem.
///
/// The second case cannot happen locally, where the subject is always `local`.
/// It becomes reachable when `platform#11` supplies real subject ids, and an
/// app that sees it is looking at a platform-side problem rather than anything
/// its caller did — worth logging as such, not reporting as "sign in".
#[must_use]
pub fn for_caller(caller: &Caller) -> Option<Namespace> {
    let subject = caller.subject()?;
    if !key_grammar_permits(subject) {
        return None;
    }
    Some(Namespace {
        // Prefixed, so a per-caller key can never collide with an app-wide key
        // an author chose.
        prefix: Some(format!("{CALLER_PREFIX}{subject}")),
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
        assert_eq!(ns.key("note"), "u.alice.note");
    }

    /// **The test that was missing.**
    ///
    /// The previous version asserted `ns.key("note") == "u/alice/note"` — what
    /// the function constructs, not what the store accepts. It passed while
    /// every call through `for_caller` failed at runtime, because `/` is not in
    /// the published grammar and the host rejected every key.
    ///
    /// A pure-function test of a value another system validates is not a test
    /// of the integration. This asserts the property that matters instead: the
    /// keys this module generates are keys `clwd:app-data` will accept.
    #[test]
    fn every_generated_key_satisfies_the_published_grammar() {
        let subjects = ["local", "alice", "u-123", "a.b.c", "x_y", "0"];
        let keys = ["note", "a", "some.key", "with_underscore", "with-dash"];

        for subject in subjects {
            let ns = for_caller(&user(subject))
                .unwrap_or_else(|| panic!("{subject:?} should yield a namespace"));
            for key in keys {
                let generated = ns.key(key);
                assert!(
                    key_grammar_permits(&generated),
                    "for_caller({subject:?}).key({key:?}) produced {generated:?}, \
                     which the app-data grammar rejects — the host will refuse it"
                );
            }
        }

        // The shared namespace passes the caller's key through, so it is only
        // as valid as what the app supplies; assert the pass-through itself.
        for key in keys {
            assert!(key_grammar_permits(&shared().key(key)));
        }
    }

    /// A subject that cannot appear in a key is refused, not mangled. Two
    /// subjects squashed into one key would put two callers in one namespace,
    /// which is a data leak rather than a formatting problem.
    #[test]
    fn a_subject_outside_the_grammar_is_refused_rather_than_reshaped() {
        for bad in [
            "Alice",
            "a/b",
            "a b",
            "user@example.com",
            "",
            "-lead",
            "trail.",
        ] {
            assert!(
                for_caller(&user(bad)).is_none(),
                "{bad:?} was accepted as a namespace subject"
            );
        }
    }

    /// The grammar check is a second implementation of a published rule, so it
    /// is asserted against the rule's own wording rather than trusted.
    #[test]
    fn the_grammar_check_matches_what_the_wit_documents() {
        // "1 to 128 bytes"
        assert!(!key_grammar_permits(""));
        assert!(key_grammar_permits(&"a".repeat(128)));
        assert!(!key_grammar_permits(&"a".repeat(129)));
        // "of a-z, 0-9, ., _ or -"
        assert!(key_grammar_permits("a0._-x"));
        assert!(!key_grammar_permits("A"));
        assert!(!key_grammar_permits("/"));
        // "may not begin or end with ., _ or -"
        for edge in [".x", "_x", "-x", "x.", "x_", "x-"] {
            assert!(!key_grammar_permits(edge), "{edge:?} was accepted");
        }
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
        assert!(ns.key("config").starts_with(CALLER_PREFIX));
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
        assert_eq!(ns.key("cursor"), "u.billing.cursor");
    }

    /// Every store failure is the app's own problem, not the caller's.
    #[test]
    fn a_store_failure_is_reported_as_internal_not_as_the_callers_fault() {
        let e: crate::Error = DataError::Unavailable.into();
        assert_eq!(e.status(), 500);
    }
}
