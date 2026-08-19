//! Firecracker-backed `Sandbox` for long-lived, stateful apps.
//!
//! No well-maintained firecracker-rs wrapper exists, so this drives the
//! Firecracker API socket directly via hyper + hyperlocal, with tokio-vsock
//! for host<->guest control. Snapshot-restore is a day-one design goal, not
//! a retrofit — it's what gets cold start under ~150ms.

pub struct MicroVmSandbox;
