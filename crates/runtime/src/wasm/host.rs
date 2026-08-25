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
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpHooks, WasiHttpView};

use crate::identity::{Caller, IdentityCtx, Role};
use crate::logs::LogStreamKind;
use crate::wasm::logs::LogSink;

wasmtime::component::bindgen!({
    inline: "
        package clwd:host@0.1.0;
        world identity-host {
            import clwd:identity/context@0.1.0;
        }
    ",
    path: "../../wit",
});

use clwd::identity::types as wit;

/// Everything one `Store` owns. Created fresh for each invocation.
pub(super) struct StoreState {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    hooks: DefaultHooks,
    table: ResourceTable,
    /// The one caller this `Store` serves. Read-only: there is no setter, and
    /// the value arrives by move at construction.
    identity: IdentityCtx,
}

#[derive(Default)]
pub(super) struct DefaultHooks;
impl WasiHttpHooks for DefaultHooks {}

impl StoreState {
    pub(super) fn new(identity: IdentityCtx, stdout: LogSink, stderr: LogSink) -> Self {
        let wasi = WasiCtxBuilder::new().stdout(stdout).stderr(stderr).build();
        Self {
            wasi,
            http: WasiHttpCtx::new(),
            hooks: DefaultHooks,
            table: ResourceTable::new(),
            identity,
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

/// Wire `clwd:identity/context` into a linker.
pub(super) fn add_identity_to_linker(
    linker: &mut wasmtime::component::Linker<StoreState>,
) -> wasmtime::Result<()> {
    use wasmtime::component::HasSelf;
    clwd::identity::types::add_to_linker::<_, HasSelf<StoreState>>(linker, |s| s)?;
    clwd::identity::context::add_to_linker::<_, HasSelf<StoreState>>(linker, |s| s)?;
    Ok(())
}
