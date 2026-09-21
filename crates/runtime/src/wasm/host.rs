//! Per-`Store` state, and the `clwd:identity/context` host implementation.
//!
//! # Why a guest cannot read another caller's identity
//!
//! The generated host trait is implemented on the `Store`'s data `T`. wasmtime
//! hands the implementation `&mut T` for the `Store` the call arrived on, so
//! `get_caller` can only read the [`IdentityCtx`] that `Store` was built with.
//!
//! There is no registry, no thread-local, and no lookup keyed by anything the
//! guest supplies. A guest cannot name another `Store`, so there is no path to
//! another identity — not one that is guarded, one that does not exist.

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

use crate::identity::{Caller, IdentityCtx, Role};
use crate::logs::LogStreamKind;
use crate::wasm::budget::Budget;
use crate::wasm::capability::{EgressHooks, EgressPolicy, StoragePolicy};
use crate::wasm::data::{AppDataStore, AppId, MIN_VALUE_LIMIT, validate_key};
use crate::wasm::limits::BudgetLimiter;
use crate::wasm::logs::LogSink;
use std::collections::HashMap;
use std::sync::Arc;

wasmtime::component::bindgen!({
    inline: "
        package clwd:host@0.1.0;
        world data-host {
            import clwd:identity/context@0.1.0;
            import clwd:secrets/broker@0.1.0;
            import clwd:app-data/store@0.1.0;
        }
    ",
    path: "../../wit",
    // Only app-data is async: it calls a store that may do I/O. Identity and
    // secrets read per-`Store` state and stay synchronous, which also leaves
    // #54's identity host untouched.
    imports: { "clwd:app-data/store": async },
});

use clwd::identity::types as wit;

/// Everything one `Store` owns. Created fresh for each invocation.
pub(super) struct StoreState {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    /// The outbound-egress allowlist, as `wasi:http` hooks. Denies by construction
    /// when the manifest granted no egress.
    hooks: EgressHooks,
    /// The storage-namespace allowlist, checked on every app-data operation.
    storage: StoragePolicy,
    table: ResourceTable,
    /// The one caller this `Store` serves. Read-only: there is no setter, and
    /// the value arrives by move at construction.
    identity: IdentityCtx,
    /// Resource caps. Lives here because `Store::limiter` resolves the limiter
    /// out of the store's data — wasmtime gives no other place to put it. It is
    /// per-`Store` like everything else in this struct.
    pub(super) limiter: BudgetLimiter,
    /// Every secret this instance may see, resolved before it started.
    ///
    /// Per-`Store` and complete: `get` reads this map and consults nothing
    /// else, so a secret absent here is not reachable rather than refused.
    secrets: HashMap<String, Vec<u8>>,
    /// The application whose data this instance reads and writes. Supplied by
    /// the host at spawn; no guest can name or alter it.
    app: AppId,
    /// Shared across instances by design — see the type's documentation. Every
    /// operation is scoped by `app`, which is why sharing is safe.
    app_data: Arc<dyn AppDataStore>,
}

impl StoreState {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        identity: IdentityCtx,
        budget: &Budget,
        secrets: HashMap<String, Vec<u8>>,
        app: AppId,
        app_data: Arc<dyn AppDataStore>,
        egress: EgressPolicy,
        storage: StoragePolicy,
        stdout: LogSink,
        stderr: LogSink,
    ) -> Self {
        let wasi = WasiCtxBuilder::new().stdout(stdout).stderr(stderr).build();
        Self {
            wasi,
            http: WasiHttpCtx::new(),
            hooks: EgressHooks::new(egress),
            storage,
            table: ResourceTable::new(),
            identity,
            limiter: BudgetLimiter::new(budget),
            secrets,
            app,
            app_data,
        }
    }
}

impl WasiView for StoreState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for StoreState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            hooks: &mut self.hooks,
            table: &mut self.table,
            ctx: &mut self.http,
        }
    }
}

impl wit::Host for StoreState {}

impl clwd::identity::context::Host for StoreState {
    fn get_caller(&mut self) -> wit::Caller {
        match &self.identity.caller {
            Caller::Anonymous => wit::Caller::Anonymous,
            Caller::User(p) => wit::Caller::User(wit::Principal {
                subject: p.subject.as_str().to_owned(),
                org: p.org.as_str().to_owned(),
                display_name: p.display_name.clone(),
                email: p.email.clone(),
            }),
            Caller::Service(s) => wit::Caller::Service(wit::ServicePrincipal {
                subject: s.subject.as_str().to_owned(),
                org: s.org.as_str().to_owned(),
                on_behalf_of: s.on_behalf_of.as_ref().map(|s| s.as_str().to_owned()),
            }),
        }
    }

    fn get_role(&mut self) -> Option<wit::Role> {
        self.identity.role.map(|role| match role {
            Role::Viewer => wit::Role::Viewer,
            Role::Editor => wit::Role::Editor,
            Role::Owner => wit::Role::Owner,
        })
    }

    fn in_group(&mut self, group: String) -> bool {
        self.identity.groups.iter().any(|g| g.as_str() == group)
    }

    fn list_groups(&mut self) -> Vec<String> {
        self.identity
            .groups
            .iter()
            .map(|g| g.as_str().to_owned())
            .collect()
    }

    /// Authorization is a control-plane call and belongs to a later issue.
    /// Failing closed is the only safe answer until it exists: an `unavailable`
    /// error is what the WIT tells callers to expect, and it is what they must
    /// already handle.
    fn authorize(
        &mut self,
        _action: String,
        _target: wit::EntityRef,
    ) -> Result<wit::Decision, wit::AuthzError> {
        Err(wit::AuthzError::Unavailable)
    }

    fn authorize_many(
        &mut self,
        _requests: Vec<wit::AuthzRequest>,
    ) -> Result<Vec<wit::Decision>, wit::AuthzError> {
        // Whole-batch failure, per the WIT: there is no partial-success shape,
        // because a caller with some answers and not others must fail closed
        // anyway.
        Err(wit::AuthzError::Unavailable)
    }
}

/// The two sinks a `Store` writes guest output into.
pub(super) fn sinks() -> (LogSink, LogSink) {
    (
        LogSink::new(LogStreamKind::Stdout),
        LogSink::new(LogStreamKind::Stderr),
    )
}

/// Wire the first-party host interfaces into a linker.
pub(super) fn add_identity_to_linker(
    linker: &mut wasmtime::component::Linker<StoreState>,
) -> wasmtime::Result<()> {
    use wasmtime::component::HasSelf;
    clwd::identity::types::add_to_linker::<_, HasSelf<StoreState>>(linker, |s| s)?;
    clwd::identity::context::add_to_linker::<_, HasSelf<StoreState>>(linker, |s| s)?;
    clwd::secrets::broker::add_to_linker::<_, HasSelf<StoreState>>(linker, |s| s)?;
    clwd::app_data::store::add_to_linker::<_, HasSelf<StoreState>>(linker, |s| s)?;
    Ok(())
}

impl clwd::secrets::broker::Host for StoreState {
    /// Reads the injected map and nothing else.
    ///
    /// There is no store to consult and no policy to evaluate: everything this
    /// instance may see was resolved before it started. `None` covers both "no
    /// such secret" and "not yours" because they are the same value, not
    /// because this function treats them alike — see DECISION 1 in
    /// `wit/deps/secrets/secrets.wit`.
    fn get(&mut self, name: String) -> Option<Vec<u8>> {
        self.secrets.get(&name).cloned()
    }
}

/// Turn a host-side failure into the interface's error, without leaking which
/// application or key was involved into the guest.
fn store_unavailable(_: crate::error::Error) -> clwd::app_data::store::Error {
    clwd::app_data::store::Error::Unavailable
}

/// Refuse an app-data operation when the app was not granted storage.
///
/// Deny-by-default and on/off: with an empty `storage_namespaces` the app may
/// not persist data at all (see [`StoragePolicy`] for why gating is boolean, not
/// per-namespace). The specifics are logged host-side under
/// `runtime.capability_denied` for the app owner; the guest gets a generic
/// `invalid-key`, never anything it can probe with (module docs,
/// `wasm/capability.rs`). The app-data interface has no dedicated "denied"
/// variant, so `invalid-key` is the closest typed, guest-handleable error — do
/// not widen it into leaking whether other keys would be accepted.
fn deny_if_storage_ungranted(
    storage: &StoragePolicy,
    key: &str,
) -> Result<(), clwd::app_data::store::Error> {
    if storage.allows(key) {
        return Ok(());
    }
    let denied = crate::error::Error::CapabilityDenied {
        detail: format!("app-data operation on {key:?} but the manifest grants no storage"),
    };
    tracing::warn!(code = denied.code(), detail = %denied, "storage access denied");
    Err(clwd::app_data::store::Error::InvalidKey(
        "this app is not granted persistent storage".to_owned(),
    ))
}

impl clwd::app_data::store::Host for StoreState {
    async fn get(&mut self, key: String) -> Result<Option<Vec<u8>>, clwd::app_data::store::Error> {
        validate_key(&key).map_err(clwd::app_data::store::Error::InvalidKey)?;
        deny_if_storage_ungranted(&self.storage, &key)?;
        self.app_data
            .get(&self.app, &key)
            .await
            .map_err(store_unavailable)
    }

    async fn set(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), clwd::app_data::store::Error> {
        validate_key(&key).map_err(clwd::app_data::store::Error::InvalidKey)?;
        deny_if_storage_ungranted(&self.storage, &key)?;
        if value.len() as u64 > MIN_VALUE_LIMIT {
            return Err(clwd::app_data::store::Error::ValueTooLarge(MIN_VALUE_LIMIT));
        }
        self.app_data
            .set(&self.app, &key, value)
            .await
            .map_err(store_unavailable)
    }

    async fn delete(&mut self, key: String) -> Result<(), clwd::app_data::store::Error> {
        validate_key(&key).map_err(clwd::app_data::store::Error::InvalidKey)?;
        deny_if_storage_ungranted(&self.storage, &key)?;
        self.app_data
            .delete(&self.app, &key)
            .await
            .map_err(store_unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deny-by-default at the app-data boundary: with no storage granted, an
    /// operation is refused and the guest sees a generic `invalid-key` — the one
    /// typed, handleable error the app-data interface offers for a refusal.
    #[test]
    fn an_ungranted_app_data_operation_is_denied() {
        let ungranted = StoragePolicy::from_manifest(&[]);
        let err = deny_if_storage_ungranted(&ungranted, "counter")
            .expect_err("empty storage grant must deny");
        assert!(matches!(err, clwd::app_data::store::Error::InvalidKey(_)));
    }

    /// A non-empty grant turns app-data on for any key shape.
    #[test]
    fn a_granted_app_may_use_app_data() {
        let granted = StoragePolicy::from_manifest(&["app".to_owned()]);
        assert!(deny_if_storage_ungranted(&granted, "counter").is_ok());
        assert!(deny_if_storage_ungranted(&granted, "u.local.note").is_ok());
    }
}
