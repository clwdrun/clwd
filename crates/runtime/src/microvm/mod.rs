//! Firecracker-backed `Sandbox` for long-lived, stateful apps.
//!
//! No well-maintained firecracker-rs wrapper exists, so this drives the
//! Firecracker API socket directly via hyper + hyperlocal, with tokio-vsock
//! for host<->guest control. Snapshot-restore is a day-one design goal, not
//! a retrofit — it's what gets cold start under ~150ms.
//!
//! The implementation is #17. This is the type it will hang off.

/// The weaker backend for caller isolation: the VM is long-lived by design, so
/// the guest agent must scope identity per request rather than per instance.
/// See the invariant on [`crate::Sandbox`].
pub struct MicroVmSandbox;
