//! One request, one `Store`, one instance.
//!
//! Everything here is per-invocation by design — see the isolation note on the
//! module above. A `Store` is built from the handle's identity, used once, and
//! dropped.

use std::sync::Arc;

use http_body_util::BodyExt;
use wasmtime::{Engine, Store, UpdateDeadline};
use wasmtime_wasi_http::p2::bindings::http::types::Scheme;
use wasmtime_wasi_http::p2::body::HyperIncomingBody;
use wasmtime_wasi_http::WasiHttpView;

use crate::call::{Body, Request, RequestId, Response};
use crate::error::{Error, Phase, Result};
use crate::identity::IdentityCtx;
use crate::wasm::budget::Budget;
use crate::wasm::cache::Prepared;
use crate::wasm::host::{self, StoreState};
use crate::wasm::limits::LimitExceeded;
use crate::wasm::logs::InstanceLogs;

/// How long a guest may run before yielding to the executor, in epoch ticks.
///
/// This bounds monopolisation, not consumption: a guest that yields every 5ms
/// still runs until its budget is spent. The final slice is trimmed to whatever
/// budget remains, so enforcement stays accurate to a single tick while yields
/// stay infrequent enough not to cost a fiber switch every millisecond.
const SLICE_TICKS: u64 = 5;

pub(super) async fn run(
    engine: &Engine,
    prepared: &Prepared,
    identity: IdentityCtx,
    budget: Budget,
    req: Request,
    request_id: Option<RequestId>,
    logs: Arc<InstanceLogs>,
) -> Result<Response> {
    let (stdout, stderr) = host::sinks();
    let mut store = Store::new(
        engine,
        StoreState::new(identity, &budget, stdout.clone(), stderr.clone()),
    );

    // Resource caps. `Store::limiter` resolves the limiter out of the store's
    // own data, so nothing is shared between guests.
    store.limiter(|state| &mut state.limiter);

    // The execution-time budget, in ticks. One tick is one millisecond, so
    // `cpu_ms` is a tick count with no conversion.
    //
    // The guest runs in slices: at each deadline the callback yields to the
    // executor and extends, until the budget is spent and it interrupts
    // instead. Yielding is fairness — one guest cannot monopolise a worker.
    // Interrupting is enforcement. They are not alternatives, and a design with
    // only the yield would let a spinning guest run forever, politely.
    //
    // Nothing here fires unless the epoch is advancing; see `epoch.rs` for why
    // that ticker is an OS thread.
    let mut remaining = budget.cpu_ticks.max(1);
    let mut slice = remaining.clamp(1, SLICE_TICKS);
    store.set_epoch_deadline(slice);
    store.epoch_deadline_callback(move |_ctx| {
        remaining = remaining.saturating_sub(slice);
        if remaining == 0 {
            // Budget spent. Raises `Trap::Interrupt`, which the error mapping
            // below turns into `runtime.deadline_exceeded` rather than a trap.
            return Ok(UpdateDeadline::Interrupt);
        }
        // Trim the last slice to what is left, so the budget is enforced to the
        // tick rather than to the slice.
        slice = remaining.clamp(1, SLICE_TICKS);
        Ok(UpdateDeadline::Yield(slice))
    });

    let proxy = prepared
        .instantiate_async(&mut store)
        .await
        .map_err(|e| classify(e, Phase::Spawn, &budget, "instantiating"))?;

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

    call.map_err(|e| classify(e, Phase::Invoke, &budget, "guest"))?;

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

/// Turn a failure out of wasm into the right error.
///
/// Three outcomes that must not be conflated, because they call for different
/// responses: the guest asked for more than it was given, the guest ran longer
/// than it was given, or the guest is broken.
///
/// The order matters. A limiter denial arrives *as* a trap carrying the typed
/// error, and an epoch interrupt arrives as `Trap::Interrupt`, so both must be
/// recognised before the fallback claims them as ordinary traps.
fn classify(error: wasmtime::Error, phase: Phase, budget: &Budget, during: &str) -> Error {
    // A denial is only ever present on the denial path, so this cannot
    // mis-attribute a guest that handled a failed grow and later trapped for an
    // unrelated reason.
    if let Some(exceeded) = error.downcast_ref::<LimitExceeded>() {
        return Error::ResourceExhausted {
            limit: exceeded.limit,
            requested: exceeded.requested,
            allowed: exceeded.allowed,
        };
    }

    // `UpdateDeadline::Interrupt` raises exactly this. Distinguishing "ran too
    // long" from "the guest panicked" is why both codes exist.
    if matches!(error.downcast_ref::<wasmtime::Trap>(), Some(wasmtime::Trap::Interrupt)) {
        return Error::DeadlineExceeded {
            phase,
            limit: budget.cpu,
        };
    }

    // Everything else is the guest's own fault, and its message is the only
    // diagnostic an app author gets.
    Error::Trap {
        message: format!("{during}: {error:?}"),
    }
}
