//! A scripted `Sandbox` for exercising the lifecycle with no wasmtime and no
//! KVM.
//!
//! Scripted rather than null, because half of what is worth testing is a
//! backend misbehaving on purpose: a bundle that will not decode, a phase that
//! overruns its deadline, an instance that dies underneath its handle.
//!
//! It also **records** what it was asked to do. That is how the caller-isolation
//! claims get tested rather than asserted: [`TestSandbox::instances`] reports
//! every caller each instance ever observed, and the invariant says that number
//! is always one.
//!
//! #20 builds its conformance suite on this rather than replacing it — the
//! builder is the extension point and the recorder is the observation point.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;

use crate::call::{Body, InvokeOptions, Request, Response, SpawnOptions};
use crate::error::{Error, Result};
use crate::handle::{Handle, InstanceId};
use crate::identity::{Caller, IdentityCtx};
use crate::logs::{LogLine, LogStream};
use crate::{BundleRef, Sandbox};

type Responder = Box<dyn Fn(&Request) -> Response + Send + Sync>;

/// What one instance was asked to do, for tests that assert on isolation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstanceRecord {
    pub instance: InstanceId,
    /// Every caller this instance has observed.
    ///
    /// The caller-isolation invariant says this is always exactly one. It is a
    /// `Vec` so that a violation is *representable* and therefore assertable —
    /// a field that could only hold one caller would make the test vacuous.
    pub callers: Vec<Caller>,
    pub invocations: u32,
    pub stopped: bool,
}

struct State {
    artifacts: HashMap<String, Vec<u8>>,
    spawn_cost: Duration,
    invoke_cost: Duration,
    logs: Vec<LogLine>,
    die_after_invokes: Option<u32>,
    responder: Responder,
    instances: Vec<InstanceRecord>,
    next_id: u64,
}

/// A `Sandbox` that does exactly what a test told it to.
pub struct TestSandbox {
    state: Mutex<State>,
}

impl std::fmt::Debug for TestSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestSandbox").finish_non_exhaustive()
    }
}

impl TestSandbox {
    pub fn builder() -> TestSandboxBuilder {
        TestSandboxBuilder {
            artifacts: HashMap::new(),
            spawn_cost: Duration::ZERO,
            invoke_cost: Duration::ZERO,
            logs: Vec::new(),
            die_after_invokes: None,
            // `builder()` lives on `Response<()>`, so it is reached through
            // `http::Response` rather than through our aliased `Response`.
            responder: Box::new(|_| {
                http::Response::builder()
                    .status(200)
                    .body(Body::empty())
                    .expect("a 200 with an empty body is always valid")
            }),
        }
    }

    /// Every instance this backend has spawned, in order.
    pub fn instances(&self) -> Vec<InstanceRecord> {
        self.state.lock().instances.clone()
    }

    fn record_mut<'a>(state: &'a mut State, id: &InstanceId) -> Option<&'a mut InstanceRecord> {
        state.instances.iter_mut().find(|r| &r.instance == id)
    }
}

/// Builds a [`TestSandbox`]. Every knob is a thing a real backend can do to you.
pub struct TestSandboxBuilder {
    artifacts: HashMap<String, Vec<u8>>,
    spawn_cost: Duration,
    invoke_cost: Duration,
    logs: Vec<LogLine>,
    die_after_invokes: Option<u32>,
    responder: Responder,
}

impl TestSandboxBuilder {
    /// Store an artifact under `bundle`, exactly as a real store would.
    ///
    /// The bytes are decoded on spawn by `clwd_bundle::decode`, so a corrupted
    /// artifact produces a genuine `bundle.*` error rather than a synthesized
    /// one — which is the only way to test that the code survives delegation.
    pub fn artifact(mut self, bundle: &BundleRef, bytes: Vec<u8>) -> Self {
        self.artifacts.insert(bundle.to_string(), bytes);
        self
    }

    /// How long a spawn "takes". Compared against the deadline rather than
    /// slept, so a deadline test is deterministic and instant.
    pub fn spawn_cost(mut self, cost: Duration) -> Self {
        self.spawn_cost = cost;
        self
    }

    /// How long an invoke "takes". See [`Self::spawn_cost`].
    pub fn invoke_cost(mut self, cost: Duration) -> Self {
        self.invoke_cost = cost;
        self
    }

    pub fn logs(mut self, logs: Vec<LogLine>) -> Self {
        self.logs = logs;
        self
    }

    /// The instance dies on its own after `n` invocations, so the next call
    /// reports `runtime.stopped`. Not use-after-stop, which does not compile.
    pub fn die_after_invokes(mut self, n: u32) -> Self {
        self.die_after_invokes = Some(n);
        self
    }

    pub fn responder<F>(mut self, responder: F) -> Self
    where
        F: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        self.responder = Box::new(responder);
        self
    }

    pub fn build(self) -> TestSandbox {
        TestSandbox {
            state: Mutex::new(State {
                artifacts: self.artifacts,
                spawn_cost: self.spawn_cost,
                invoke_cost: self.invoke_cost,
                logs: self.logs,
                die_after_invokes: self.die_after_invokes,
                responder: self.responder,
                instances: Vec::new(),
                next_id: 0,
            }),
        }
    }
}

/// Would `cost` overrun the budget in `opts`? Checked rather than slept.
fn overruns(deadline: Option<crate::call::Deadline>, cost: Duration) -> bool {
    deadline.is_some_and(|d| cost > d.remaining())
}

#[async_trait]
impl Sandbox for TestSandbox {
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle> {
        let mut state = self.state.lock();

        let bytes = state
            .artifacts
            .get(&bundle.to_string())
            .cloned()
            .ok_or(Error::BundleNotFound { bundle })?;

        // A real backend decodes what the digest commits to, and takes the
        // app's capabilities from that manifest — never from `ctx`.
        clwd_bundle::decode(&bytes, None)?;

        if overruns(opts.deadline, state.spawn_cost) {
            return Err(Error::SpawnDeadlineExceeded {
                limit: state.spawn_cost,
            });
        }

        let instance = InstanceId::new(format!("test-{}", state.next_id));
        state.next_id += 1;
        state.instances.push(InstanceRecord {
            instance: instance.clone(),
            callers: vec![ctx.caller.clone()],
            invocations: 0,
            stopped: false,
        });

        Ok(Handle::new(ctx, instance))
    }

    async fn invoke(&self, handle: &Handle, req: Request, opts: InvokeOptions) -> Result<Response> {
        let mut state = self.state.lock();

        let cost = state.invoke_cost;
        let limit = state.die_after_invokes;
        let record = Self::record_mut(&mut state, handle.instance()).ok_or(Error::Stopped {})?;
        if record.stopped {
            return Err(Error::Stopped {});
        }
        if limit.is_some_and(|n| record.invocations >= n) {
            record.stopped = true;
            return Err(Error::Stopped {});
        }
        record.invocations += 1;

        if overruns(opts.deadline, cost) {
            return Err(Error::InvokeDeadlineExceeded { limit: cost });
        }

        Ok((state.responder)(&req))
    }

    async fn logs_stream(&self, handle: &Handle) -> Result<LogStream> {
        let state = self.state.lock();
        let live = state
            .instances
            .iter()
            .any(|r| &r.instance == handle.instance() && !r.stopped);
        if !live {
            return Err(Error::Stopped {});
        }
        Ok(LogStream::new(Scripted(state.logs.clone().into_iter())))
    }

    async fn stop(&self, handle: Handle) -> Result<()> {
        let mut state = self.state.lock();
        if let Some(record) = Self::record_mut(&mut state, handle.instance()) {
            record.stopped = true;
        }
        Ok(())
    }
}

/// A stream over a fixed list of lines.
struct Scripted(std::vec::IntoIter<LogLine>);

impl Stream for Scripted {
    type Item = Result<LogLine>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.next().map(Ok))
    }
}
