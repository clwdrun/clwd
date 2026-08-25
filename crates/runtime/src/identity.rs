//! Who is calling. Mirrors `wit/deps/identity/identity.wit`.
//!
//! Nothing here is app-level. Capabilities live in the bundle manifest, sealed
//! by the digest `spawn` is given, and are deliberately *not* reachable through
//! this module: two sources of truth for one app's permissions is the
//! silent-permission failure the bundle format exists to prevent.
//!
//! The split is: anything that varies per **caller** is a [`Role`] and lives
//! here; anything that varies per **app** is a capability and lives in the
//! manifest.

use std::fmt;

/// Opaque, stable, org-unique. Not an email — emails get renamed and reused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubjectId(String);

/// The organization a subject belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OrgId(String);

/// A group within an organization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(String);

macro_rules! opaque_id {
    ($($ty:ident),* $(,)?) => {$(
        impl $ty {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    )*};
}
opaque_id!(SubjectId, OrgId, GroupId);

/// A human caller.
///
/// `Debug` is implemented by hand rather than derived: `email` and
/// `display_name` are personal data, and a derived `Debug` is how they end up
/// in a log line nobody meant to write.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Principal {
    pub subject: SubjectId,
    pub org: OrgId,
    pub display_name: Option<String>,
    pub email: Option<String>,
}

impl Principal {
    pub fn new(subject: SubjectId, org: OrgId) -> Self {
        Self {
            subject,
            org,
            display_name: None,
            email: None,
        }
    }
}

impl fmt::Debug for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Principal")
            .field("subject", &self.subject)
            .field("org", &self.org)
            .field("display_name", &redacted(&self.display_name))
            .field("email", &redacted(&self.email))
            .finish()
    }
}

fn redacted(value: &Option<String>) -> &'static str {
    match value {
        Some(_) => "<redacted>",
        None => "None",
    }
}

/// A non-human caller: another app, or an agent holding a delegated grant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServicePrincipal {
    pub subject: SubjectId,
    pub org: OrgId,
    pub on_behalf_of: Option<SubjectId>,
}

impl ServicePrincipal {
    pub fn new(subject: SubjectId, org: OrgId) -> Self {
        Self {
            subject,
            org,
            on_behalf_of: None,
        }
    }
}

/// The caller for one instance's whole lifetime.
///
/// Deliberately **not** `#[non_exhaustive]`. A new kind of caller is a security
/// question, and every consumer should be forced to answer it: adding a variant
/// here SHOULD break every `match` rather than fall into a wildcard arm that
/// treats an unrecognized principal as something already handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// Reachable, and not an oversight: a public share link has no session.
    ///
    /// Whether an app may be reached anonymously is control-plane policy. This
    /// type represents the state because the state exists in the world; a type
    /// that cannot express a reachable state only moves the handling somewhere
    /// less visible.
    Anonymous,
    User(Principal),
    Service(ServicePrincipal),
}

/// The caller's role on this app, resolved from share grants.
///
/// Not `#[non_exhaustive]`, for the same reason as [`Caller`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Editor,
    Owner,
}

/// Everything a backend is told about the caller an instance serves.
///
/// Consumed by `Sandbox::spawn`, and never seen again: `invoke` takes no
/// identity, so there is no way to express running an existing instance as a
/// different caller. See the caller-isolation invariant on the trait.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityCtx {
    pub caller: Caller,
    /// `None` for anonymous callers, matching `get-role` in the WIT.
    pub role: Option<Role>,
    pub groups: Vec<GroupId>,
}

impl IdentityCtx {
    /// A context for `caller` with no role and no groups.
    ///
    /// This type is `#[non_exhaustive]`, so it cannot be built with a struct
    /// literal outside this crate — including with `..Default::default()`.
    /// Build it here and assign the fields you need:
    ///
    /// ```
    /// # use clwd_runtime::identity::*;
    /// let mut ctx = IdentityCtx::new(Caller::Anonymous);
    /// ctx.groups.push(GroupId::new("everyone"));
    /// ```
    pub fn new(caller: Caller) -> Self {
        Self {
            caller,
            role: None,
            groups: Vec::new(),
        }
    }

    /// The subject this context identifies, or `None` when anonymous.
    pub fn subject(&self) -> Option<&SubjectId> {
        match &self.caller {
            Caller::Anonymous => None,
            Caller::User(p) => Some(&p.subject),
            Caller::Service(s) => Some(&s.subject),
        }
    }
}
