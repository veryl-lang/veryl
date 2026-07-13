//! Driving of user-defined verification components: host services, the
//! runtime that stages inputs and fires hooks around each event, and
//! loading of component libraries.

pub mod host;
pub mod loader;
pub mod runtime;
#[cfg(not(target_family = "wasm"))]
pub mod wasm;
