//! The handle a backend returns from `spawn`, and the identity it is bound to.

use crate::identity::{Caller, IdentityCtx};

/// A backend's name for one running instance. Format is the backend's business.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A running instance, bound to exactly one caller.
///
/// # What the type enforces
///
/// - **Not `Clone`, not `Copy`.** A handle is one instance, held in one place.
/// - **Only constructible from an [`IdentityCtx`]**, which [`Handle::new`]
///   consumes. Every handle in existence was minted against exactly one caller.
/// - **`Sandbox::stop` takes it by value.** Using a handle after stopping it is
///   a compile error, not a runtime one.
/// - **`Sandbox::invoke` takes no identity.** There is no parameter, field or
///   setter through which a second caller could reach an existing instance —
///   the leak cannot be written in terms of this API.
///
/// # What it does not enforce
///
/// A backend that pools instances internally can still return two handles that
/// alias one guest. No signature can see inside an implementation; that
/// obligation is stated on the `Sandbox` trait, where a backend author reads it.
#[derive(Debug)]
pub struct Handle {
    instance: InstanceId,
    identity: IdentityCtx,
}

impl Handle {
    /// Mint a handle for `instance`, bound to `ctx`.
    ///
    /// Consuming the context is the point: it cannot then be handed to a second
    /// instance, and there is no way to build a handle without one.
    pub fn new(ctx: IdentityCtx, instance: InstanceId) -> Self {
        Self {
            instance,
            identity: ctx,
        }
    }

    pub fn instance(&self) -> &InstanceId {
        &self.instance
    }

    /// The caller this instance serves, for its whole lifetime.
    pub fn caller(&self) -> &Caller {
        &self.identity.caller
    }

    /// Read-only. There is deliberately no `identity_mut`: a mutable borrow
    /// would be exactly the setter this design exists to withhold.
    pub fn identity(&self) -> &IdentityCtx {
        &self.identity
    }
}
