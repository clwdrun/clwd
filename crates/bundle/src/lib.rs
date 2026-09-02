//! The clwd bundle format, version 1.
//!
//! `docs/spec/bundle-v1.md` is authoritative. Where this crate and that
//! document disagree, the document is right and this crate has a bug.
//!
//! No async, no wasmtime, no I/O of its own: the caller supplies bytes and
//! receives bytes. The platform builder depends on this across the repository
//! seam and must not acquire a runtime in order to read a manifest.

pub mod decode;
pub mod digest;
pub mod encode;
pub mod error;
pub mod manifest;
pub mod path;
pub mod tar;

pub use decode::{Bundle, decode};
pub use digest::{BundleRef, Digest};
pub use encode::{Encoded, File, build, encode};
pub use error::Error;
pub use manifest::{Capabilities, Limits, Manifest, Member, Runtime};

pub type Result<T> = std::result::Result<T, Error>;
