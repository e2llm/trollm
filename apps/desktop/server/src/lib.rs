//! Linux desktop application host for the shared trollm runtime.

mod connections;
mod error;
mod flows;
pub mod http;
mod models;
mod runs;
mod session;
mod vault;

pub use http::{router, Origins};
pub use runs::{backend, Host};
