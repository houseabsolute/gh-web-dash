// This crate is a library only so the integration tests in tests/ can drive
// it — nothing consumes it as an API, and it is not published. Documenting an
// `# Errors` or `# Panics` section on every public function would be
// boilerplate written for a reader who does not exist.
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

pub mod auth;
pub mod config;
pub mod filter;
pub mod github;
pub mod inclusion;
pub mod server;
pub mod store;
pub mod sync;
