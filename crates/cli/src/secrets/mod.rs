//! Local secret management for development.
//!
//! The store is a file; see [`store`] for what that protects against and what
//! it does not. The types in [`value`] are what keeps a secret from reaching a
//! log line by accident.

pub mod store;
pub mod value;

pub use store::FileStore;
pub use value::{AppName, SecretName, SecretValue};
