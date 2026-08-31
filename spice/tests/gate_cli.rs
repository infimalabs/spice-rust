#![cfg(unix)]

//! The Git-process gate fixture was retired.
//!
//! Gate policy is covered at its library boundary, while selected CLI contracts
//! remain covered by `gate_selection_cli` and the commit worker tests.

#[path = "gate_cli/fixture.rs"]
mod fixture;
