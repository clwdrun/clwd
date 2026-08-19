//! `Sandbox` is the one abstraction the control plane is allowed to see.
//! Nothing above this crate should branch on which backend is running.

use async_trait::async_trait;

pub struct BundleRef;
pub struct IdentityCtx;
pub struct Handle;
pub struct Request;
pub struct Response;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sandbox error: {0}")]
    Sandbox(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[async_trait]
pub trait Sandbox {
    async fn start(&self, bundle: BundleRef, ctx: IdentityCtx) -> Result<Handle>;
    async fn call(&self, h: &Handle, req: Request) -> Result<Response>;
    async fn stop(&self, h: Handle) -> Result<()>;
}

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "microvm")]
pub mod microvm;
