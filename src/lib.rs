//! Tokotchi internals, exposed as a library so integration tests can exercise the
//! ledger scan and pet model directly. The `tokotchi` binary (src/main.rs) is a thin
//! CLI over these modules.

pub mod ledger;
pub mod level_cli;
pub mod model;
pub mod tui;
