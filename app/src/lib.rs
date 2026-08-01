//! The browser body.
//!
//! This crate compiles to wasm and is embedded in the appliance that serves
//! it. It depends only on `wire`, so the socket is its complete system
//! boundary: journal, board, fold, lane, and trust machinery are unreachable
//! from browser code, and a server field the app needs but wire lacks is a
//! compile-time contract gap rather than an internal import.
//!
//! Two floors live here. [`socket`] admits bytes against one current opening,
//! and [`state`] holds what was admitted. The join between them is a type:
//! admission yields [`Admitted`], and state accepts nothing else, so "the
//! browser believes only what crossed the door" is checked at compile time.
//! Neither floor constructs authority: read panes append nothing, and steer
//! and grant panes submit ordinary commands through the resident.

pub mod socket;
pub mod state;

pub use socket::{Admitted, Session};
pub use state::{Applied, BrowserState, CausalLink, ChainEnd, ChainWalk};
