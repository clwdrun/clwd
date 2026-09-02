//! `clwd build` — source tree to deployable bundle.
//!
//! The pipeline is subprocess-bound: `cargo build --target wasm32-wasip2` takes
//! tens of seconds and everything this module does around it takes single-digit
//! milliseconds. Optimising the Rust here would be measuring the wrong thing.

pub mod manifest;
pub mod size;
pub mod toolchain;

pub use manifest::Source;
pub use size::{BundleSize, ComponentSize, Verdict};
pub use toolchain::Toolchain;
