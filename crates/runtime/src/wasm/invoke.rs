//! One request, one `Store`, one instance.
//!
//! Everything here is per-invocation by design — see the isolation note on the
//! module above. A `Store` is built from the handle's identity, used once, and
//! dropped.

use std::sync::Arc;

use http_body_util::BodyExt;
use wasmtime::{Engine, Store, UpdateDeadline};
use wasmtime_wasi_http::WasiHttpView;
use wasmtime_wasi_http::p2::bindings::http::types::Scheme;
use wasmtime_wasi_http::p2::body::HyperIncomingBody;

use crate::call::{Body, MAX_RESPONSE_BODY, Request, RequestId, Response};
use crate::error::{Error, Result};
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

/// Everything one instance is, gathered at spawn and handed to each of its
/// invocations. Grouped rather than passed as loose parameters because they
/// travel together and always will: each addition here is another thing an
/// instance *is*, not another argument the caller chooses.
pub(super) struct InstanceContext {
    pub(super) identity: IdentityCtx,
    pub(super) budget: Budget,
    pub(super) secrets: std::collections::HashMap<String, Vec<u8>>,
    pub(super) app: crate::wasm::data::AppId,
    pub(super) app_data: Arc<dyn crate::wasm::data::AppDataStore>,
    pub(super) egress: crate::wasm::capability::EgressPolicy,
    pub(super) storage: crate::wasm::capability::StoragePolicy,
}

pub(super) async fn run(
    engine: &Engine,
    prepared: &Prepared,
    context: InstanceContext,
    req: Request,
    request_id: Option<RequestId>,
    logs: Arc<InstanceLogs>,
) -> Result<Response> {
    let InstanceContext {
        identity,
        budget,
        secrets,
        app,
        app_data,
        egress,
        storage,
    } = context;
    let (stdout, stderr) = host::sinks();
    let mut store = Store::new(
        engine,
        StoreState::new(
            identity,
            &budget,
            secrets,
            app,
            app_data,
            egress,
            storage,
            stdout.clone(),
            stderr.clone(),
        ),
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
            // below turns into `runtime.cpu_budget_exceeded` rather than a trap.
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
        .map_err(|e| classify(e, &budget, "instantiating"))?;

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

    // The guest's `handle` and the read of its response run together, not one
    // after the other. The body crosses a bounded channel (wasi-http buffers one
    // 1 MiB chunk), so a guest writing more than that blocks until the host
    // reads; awaiting `handle` first waits on a write that is waiting on us, and
    // every response over about 2 MiB hangs until the deadline.
    let read = tokio::spawn(async move {
        let response = rx
            .await
            .map_err(|_| Error::Internal {
                source: "guest produced no response".into(),
            })?
            .map_err(|e| Error::Internal {
                source: format!("guest response error: {e}").into(),
            })?;

        let (parts, body) = response.into_parts();
        // One aggregation. A single-frame body passes through without a copy;
        // a multi-frame body is concatenated once. `Limited` stops reading at
        // the cap, so a guest never gets more host memory than that, and
        // dropping the body on the way out fails the guest's next write.
        let collected = http_body_util::Limited::new(body, MAX_RESPONSE_BODY)
            .collect()
            .await
            .map_err(|e| {
                if e.is::<http_body_util::LengthLimitError>() {
                    Error::ResponseTooLarge {
                        allowed: MAX_RESPONSE_BODY as u64,
                    }
                } else {
                    Error::Internal {
                        source: format!("reading response body: {e}").into(),
                    }
                }
            })?
            .to_bytes();
        Ok::<_, Error>(Response::from_parts(parts, Body::from(collected)))
    });
    let call = proxy
        .wasi_http_incoming_handler()
        .call_handle(&mut store, incoming, outparam)
        .await;
    // The response outparam and the body writer live in the store's table. A
    // guest that trapped, or returned without responding, leaves them there,
    // and the read would wait on them forever; dropping the store closes them.
    drop(store);
    let read = read.await.unwrap_or_else(|e| {
        Err(Error::Internal {
            source: format!("response read task: {e}").into(),
        })
    });

    // Publish whatever the guest wrote, whether or not it succeeded — a
    // trapping guest's last lines are the ones worth having.
    let mut lines = stdout.drain(request_id.as_ref());
    lines.extend(stderr.drain(request_id.as_ref()));
    logs.publish(lines);

    match (call, read) {
        // The cap is the cause even when the guest then fails on its closed
        // body stream, so it wins over whatever the guest did next.
        (_, Err(e @ Error::ResponseTooLarge { .. })) => Err(e),
        (Err(e), _) => Err(classify(e, &budget, "guest")),
        (Ok(()), read) => read,
    }
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
///
/// An epoch interrupt is always `runtime.cpu_budget_exceeded`, whether it fires
/// while instantiating or while handling the request: the budget is the
/// manifest's `cpu_ms` either way. The caller's spawn and invoke deadlines are
/// wall-clock timeouts in `mod.rs` and never pass through here.
fn classify(error: wasmtime::Error, budget: &Budget, during: &str) -> Error {
    // A denial is only ever present on the denial path, so this cannot
    // mis-attribute a guest that handled a failed grow and later trapped for an
    // unrelated reason.
    if let Some(exceeded) = error.downcast_ref::<LimitExceeded>() {
        return exceeded.into_error();
    }

    // `UpdateDeadline::Interrupt` raises exactly this. Distinguishing "ran too
    // long" from "the guest panicked" is why both codes exist.
    if matches!(
        error.downcast_ref::<wasmtime::Trap>(),
        Some(wasmtime::Trap::Interrupt)
    ) {
        return Error::CpuBudgetExceeded { limit: budget.cpu };
    }

    // Everything else is the guest's own fault, and its message is the only
    // diagnostic an app author gets.
    Error::Trap {
        message: format!("{during}: {error:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn budget() -> Budget {
        Budget {
            cpu_ticks: 50,
            cpu: Duration::from_millis(50),
            memory_bytes: 4 * 1024 * 1024,
        }
    }

    /// A limiter denial arrives as a trap carrying the typed error, so it must
    /// be recognised before the fallback claims it.
    ///
    /// Invariant: RT-6.
    #[test]
    fn a_memory_denial_maps_to_memory_limit_exceeded() {
        let error = wasmtime::Error::from(LimitExceeded::Memory {
            requested: 8,
            allowed: 4,
        });
        let mapped = classify(error, &budget(), "guest");
        assert_eq!(mapped.code(), "runtime.memory_limit_exceeded");
        assert!(!mapped.retryable());
    }

    /// The table cap, driven through the real limiter and then `classify`.
    ///
    /// **Not end-to-end.** No guest can drive this: stable Rust cannot grow a
    /// funcref table from a guest, so the reference fixture has no route that
    /// reaches the cap. This exercises everything from the limiter's decision
    /// to the code a caller sees, but not the wasm instruction that would
    /// trigger it.
    ///
    /// Invariant: RT-7.
    #[test]
    fn the_table_cap_maps_to_table_limit_exceeded() {
        use crate::wasm::limits::{BudgetLimiter, MAX_TABLE_ELEMENTS};
        use wasmtime::ResourceLimiter;

        let mut limiter = BudgetLimiter::new(&budget());
        assert!(
            limiter
                .table_growing(0, MAX_TABLE_ELEMENTS, None)
                .expect("at the cap is allowed"),
            "growing to exactly the cap is permitted"
        );
        let denial = limiter
            .table_growing(0, MAX_TABLE_ELEMENTS + 1, None)
            .expect_err("one past the cap is denied");

        let mapped = classify(denial, &budget(), "guest");
        assert_eq!(mapped.code(), "runtime.table_limit_exceeded");
        assert!(
            !mapped.retryable(),
            "a fixed host cap does not move on retry"
        );
    }

    /// `UpdateDeadline::Interrupt` raises `Trap::Interrupt`. It is the sealed
    /// CPU budget wherever it fires, so instantiation and the handler map to the
    /// same code; the caller's wall-clock deadlines are separate codes raised in
    /// `mod.rs`.
    ///
    /// Invariant: RT-5.
    #[test]
    fn an_epoch_interrupt_maps_to_cpu_budget_exceeded() {
        for during in ["instantiating", "guest"] {
            let mapped = classify(wasmtime::Trap::Interrupt.into(), &budget(), during);
            assert_eq!(mapped.code(), "runtime.cpu_budget_exceeded", "{during}");
            assert!(!mapped.retryable(), "the same work hits the same budget");
        }
    }

    /// The fallback must not over-claim. Another trap kind is the guest's own
    /// fault and keeps its message.
    #[test]
    fn any_other_trap_stays_a_trap() {
        let mapped = classify(
            wasmtime::Trap::UnreachableCodeReached.into(),
            &budget(),
            "guest",
        );
        assert_eq!(mapped.code(), "runtime.trap");
        // The guest's own message survives: it is the only diagnostic an app
        // author gets. `Display` renders the trap in prose, not as the Rust
        // variant name.
        assert!(
            mapped.to_string().contains("unreachable"),
            "the trap message must survive the mapping: {mapped}"
        );
    }
}
