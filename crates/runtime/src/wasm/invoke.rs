//! One request, one `Store`, one instance.
//!
//! Everything here is per-invocation by design — see the isolation note on the
//! module above. A `Store` is built from the handle's identity, used once, and
//! dropped.

use std::sync::Arc;

use http_body_util::BodyExt;
use wasmtime::{Engine, Store};
use wasmtime_wasi_http::p2::bindings::http::types::Scheme;
use wasmtime_wasi_http::p2::body::HyperIncomingBody;
use wasmtime_wasi_http::WasiHttpView;

use crate::call::{Body, Request, RequestId, Response};
use crate::error::{Error, Result};
use crate::identity::IdentityCtx;
use crate::wasm::cache::Prepared;
use crate::wasm::host::{self, StoreState};
use crate::wasm::logs::InstanceLogs;

pub(super) async fn run(
    engine: &Engine,
    prepared: &Prepared,
    identity: IdentityCtx,
    req: Request,
    request_id: Option<RequestId>,
    logs: Arc<InstanceLogs>,
) -> Result<Response> {
    let (stdout, stderr) = host::sinks();
    let mut store = Store::new(
        engine,
        StoreState::new(identity, stdout.clone(), stderr.clone()),
    );

    // `Config::epoch_interruption(true)` starts every Store's deadline at zero,
    // so WITHOUT THIS LINE every guest call traps immediately with
    // "wasm trap: interrupt" — and the failure reads like a wasmtime bug rather
    // than a missing default. #17 replaces this with a real budget; until then
    // the host-level timeout below is what bounds a call.
    store.set_epoch_deadline(u64::MAX);

    let proxy = prepared
        .instantiate_async(&mut store)
        .await
        .map_err(|e| trap(e, "instantiating"))?;

    let (parts, body) = req.into_parts();
    // `Full` hands the `Bytes` out as a single frame: the buffer is moved, not
    // copied.
    let body: HyperIncomingBody = http_body_util::Full::new(body.into_bytes())
        .map_err(|e: std::convert::Infallible| match e {})
        .boxed_unsync();
    let hyper_req = hyper::Request::from_parts(parts, body);

    let scheme = match hyper_req.uri().scheme_str() {
        Some("https") => Scheme::Https,
        _ => Scheme::Http,
    };

    let incoming = WasiHttpView::http(store.data_mut())
        .new_incoming_request(scheme, hyper_req)
        .map_err(|e| Error::Internal {
            source: format!("building incoming request: {e}").into(),
        })?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let outparam = WasiHttpView::http(store.data_mut())
        .new_response_outparam(tx)
        .map_err(|e| Error::Internal {
            source: format!("building response outparam: {e}").into(),
        })?;

    let call = proxy
        .wasi_http_incoming_handler()
        .call_handle(&mut store, incoming, outparam)
        .await;

    // Publish whatever the guest wrote, whether or not it succeeded — a
    // trapping guest's last lines are the ones worth having.
    let mut lines = stdout.drain(request_id.as_ref());
    lines.extend(stderr.drain(request_id.as_ref()));
    logs.publish(lines);

    call.map_err(|e| trap(e, "guest"))?;

    let response = rx
        .await
        .map_err(|_| Error::Internal {
            source: "guest produced no response".into(),
        })?
        .map_err(|e| Error::Internal {
            source: format!("guest response error: {e}").into(),
        })?;

    let (parts, body) = response.into_parts();
    // One aggregation. A single-frame body passes through without a copy; a
    // multi-frame body is concatenated once.
    let collected = body
        .collect()
        .await
        .map_err(|e| Error::Internal {
            source: format!("reading response body: {e}").into(),
        })?
        .to_bytes();

    Ok(Response::from_parts(parts, Body::from(collected)))
}

/// A guest trap keeps its message: it is the only diagnostic an app author has.
fn trap(error: wasmtime::Error, during: &str) -> Error {
    Error::Trap {
        message: format!("{during}: {error:?}"),
    }
}
