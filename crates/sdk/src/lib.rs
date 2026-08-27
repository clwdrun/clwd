//! What app code links against.
//!
//! The `clwd:app` world is generated from `wit/` at compile time by
//! [`wit_bindgen::generate!`] — there is no checked-in generated code, and so
//! nothing that can drift from the WIT it came from. Editing `wit/` and
//! rebuilding is the whole regeneration story.
//!
//! # What an app gets
//!
//! - [`identity`] — who is calling. Ambient: no request argument to thread.
//! - [`secrets`] — the credentials this app was granted, by name.
//! - [`app_data`] — a key-value namespace **belonging to the app, not to the
//!   caller**. Every caller of one app reads and writes the same data. Anything
//!   personal must be keyed by the caller's subject explicitly; see the
//!   decision block in `wit/deps/app-data/app-data.wit`.
//! - `wasi:http/proxy` — the request and response types the handler serves.

#![no_std]

extern crate alloc;

wit_bindgen::generate!({
    path: "../../wit",
    world: "app",
    generate_all,
    // The guest links this; nothing here should pull in a runtime.
    pub_export_macro: true,
});

pub use clwd::app_data;
pub use clwd::identity;
pub use clwd::secrets;
