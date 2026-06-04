//! lib.rs — Public re-exports for integration tests and external consumers.
//!
//! `main.rs` remains the binary entry point; this crate root exposes the
//! internal module tree so that `tests/*.rs` integration tests can import
//! e.g. `kryneth_gateway::api::routes::create_router`.

pub mod api;
pub mod domain;
pub mod error;
pub mod infrastructure;
pub mod usecases;
