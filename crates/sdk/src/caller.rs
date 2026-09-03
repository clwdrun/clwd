//! Who is calling.
//!
//! Identity is ambient: there is no request argument to thread, because the
//! host resolves it before the guest runs and an instance sees exactly one
//! caller for its whole lifetime. See the DECISION block in
//! `wit/deps/identity/identity.wit` for why that invariant exists and what it
//! costs the runtime to hold.
//!
//! # Anonymous is a kind of caller, not a missing one
//!
//! [`Caller`] is an enum with an [`Anonymous`](Caller::Anonymous) variant, and
//! this crate deliberately offers **no** convenience that flattens it — no
//! `subject() -> Option<String>`, no empty-string default.
//!
//! An `Option` would teach the wrong thing. An author who sees one reaches for
//! `unwrap_or_default()`, and now an unauthenticated caller is a user whose
//! subject is `""` — which will compare equal to another such caller, key an
//! app-data entry, and read like a real identity everywhere downstream. The
//! failure is silent and it is an authorization bug.
//!
//! Matching costs three lines and makes the decision visible at the point where
//! it is being made. `clwd run` chose `local@local` over anonymous for local
//! runs on the same reasoning: a distinct value rather than an absent one.

use alloc::string::String;
use alloc::vec::Vec;

use crate::clwd::identity::types as wit;

/// A person, identified by the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct User {
    /// Stable, unique, and the right thing to key data by.
    pub subject: String,
    /// The organisation this call is scoped to.
    pub org: String,
    /// For showing to humans. May be absent, and may change.
    pub display_name: Option<String>,
    /// May be absent, and is not a stable identifier — people change addresses.
    /// Key by [`subject`](Self::subject).
    pub email: Option<String>,
}

/// Another service calling this app, possibly for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Service {
    /// The calling service's own identity.
    pub subject: String,
    /// The organisation this call is scoped to.
    pub org: String,
    /// The person the service is acting for, when it is acting for one.
    ///
    /// Present means "this service is doing something on that person's
    /// behalf"; absent means the service is acting for itself.
    pub on_behalf_of: Option<String>,
}

/// Who is calling this app.
///
/// Three kinds, never collapsed. See the module docs for why there is no
/// convenience that turns this into an `Option`.
///
/// # Deliberately not `#[non_exhaustive]`
///
/// The rest of this crate marks growable public types `#[non_exhaustive]`, and
/// this enum does not, on purpose. `#[non_exhaustive]` forces every downstream
/// `match` to carry a `_` arm — which is precisely the wildcard an anonymous
/// caller would fall into, silently, undoing the reason this is an enum rather
/// than an `Option`.
///
/// Adding a variant here is therefore a breaking change. That is the correct
/// cost: a new kind of caller is exactly the thing every app must be made to
/// look at, and a compile error is the cheapest possible way to be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// No identity was established.
    ///
    /// Representable because a share link has no session. The platform denies
    /// it for MVP, so an app will not see this today — but the world admits it,
    /// and an app that handles it explicitly now will not need changing when it
    /// becomes reachable.
    Anonymous,
    /// A person.
    User(User),
    /// Another service.
    Service(Service),
}

/// What this caller may do in the current organisation.
///
/// Not `#[non_exhaustive]`, for the same reason as [`Caller`]: a new role is a
/// permission level every app has to place relative to the ones it knows, and a
/// wildcard arm would place it silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Editor,
    Owner,
}

/// Who is calling.
///
/// ```ignore
/// match clwd_sdk::caller() {
///     Caller::User(u) => greet(&u.subject),
///     Caller::Service(s) => audit(&s.subject),
///     Caller::Anonymous => Response::new(401).send(out, b"sign in"),
/// }
/// ```
#[must_use]
pub fn caller() -> Caller {
    match crate::clwd::identity::context::get_caller() {
        wit::Caller::Anonymous => Caller::Anonymous,
        wit::Caller::User(p) => Caller::User(User {
            subject: p.subject,
            org: p.org,
            display_name: p.display_name,
            email: p.email,
        }),
        wit::Caller::Service(s) => Caller::Service(Service {
            subject: s.subject,
            org: s.org,
            on_behalf_of: s.on_behalf_of,
        }),
    }
}

/// This caller's role, if they have one.
///
/// `None` means no role in this organisation — which is not the same as
/// [`Role::Viewer`], and an app that treats it as read access has granted
/// something nobody assigned.
#[must_use]
pub fn role() -> Option<Role> {
    crate::clwd::identity::context::get_role().map(|r| match r {
        wit::Role::Viewer => Role::Viewer,
        wit::Role::Editor => Role::Editor,
        wit::Role::Owner => Role::Owner,
    })
}

/// Whether this caller belongs to `group`.
#[must_use]
pub fn in_group(group: &str) -> bool {
    crate::clwd::identity::context::in_group(group)
}

/// Every group this caller belongs to.
#[must_use]
pub fn groups() -> Vec<String> {
    crate::clwd::identity::context::list_groups()
}

impl Caller {
    /// The caller's subject id, for a caller that has one.
    ///
    /// **Not a general-purpose accessor.** It exists for logging and for keying
    /// app-data, where an anonymous caller genuinely has nothing to key by and
    /// the caller of this function has to decide what that means. It returns
    /// `Option` because `Anonymous` is a real answer here, not because identity
    /// is optional — see the module docs.
    ///
    /// Do not use it for authorization. `match` on the variant instead, so the
    /// anonymous case is a branch someone wrote rather than a `None` someone
    /// defaulted away.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::Anonymous => None,
            Self::User(u) => Some(&u.subject),
            Self::Service(s) => Some(&s.subject),
        }
    }
}
