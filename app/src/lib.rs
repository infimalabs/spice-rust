//! The browser body.
//!
//! This crate compiles to wasm and is embedded in the appliance that serves
//! it. It depends only on `wire`, so the socket is its complete system
//! boundary: journal, board, fold, lane, and trust machinery are unreachable
//! from browser code, and a server field the app needs but wire lacks is a
//! compile-time contract gap rather than an internal import.
//!
//! Three floors live here. [`socket`] admits bytes against one current
//! opening, [`state`] holds what was admitted, and [`pane`] presents it. The
//! join between the first two is a type: admission yields [`Admitted`], and
//! state accepts nothing else, so "the browser believes only what crossed the
//! door" is checked at compile time. The join to the third is the same one:
//! a [`Principal`] comes from an admitted acceptance and from nowhere else,
//! so a pane's authority class is the resident's answer rather than the app's
//! assertion.
//!
//! No floor constructs authority. Read panes append nothing, and steer and
//! grant panes build ordinary commands the resident authenticates for itself.

pub mod pane;
pub mod socket;
pub mod state;

pub use pane::{Declined, GrantPane, Principal, ReadPane, SteerPane, settled};
pub use socket::{Admitted, Session};
pub use state::{Applied, BrowserState, CausalLink, ChainEnd, ChainWalk};
