//! The browser body.
//!
//! This crate compiles to wasm and is embedded in the appliance that serves
//! it. It depends only on `wire`, so the socket is its complete system
//! boundary: journal, board, fold, lane, and trust machinery are unreachable
//! from browser code, and a server field the app needs but wire lacks is a
//! compile-time contract gap rather than an internal import.
//!
//! Four floors live here. [`socket`] admits bytes against one current
//! opening, [`state`] holds what was admitted, [`pane`] presents it, and
//! [`view`] says which of the four axes a pane answers on. The join between
//! the first two is a type: admission yields [`Admitted`], and state accepts
//! nothing else, so "the browser believes only what crossed the door" is
//! checked at compile time. The join to the third is the same one: a
//! [`Principal`] comes from an admitted acceptance and from nowhere else, so a
//! pane's authority class is the resident's answer rather than the app's
//! assertion. The fourth join is a closed vocabulary: a [`Pane`] names its
//! [`Axis`] through an exhaustive match, so a pane with no axis does not
//! compile rather than rendering unclassified.
//!
//! No floor constructs authority. Read panes append nothing, and steer and
//! grant panes build ordinary commands the resident authenticates for itself.
//! No floor constructs provenance either: a fold-backed answer is held as
//! [`Answer::Derived`] wearing the fold, generation, and cursor it arrived
//! with, a head-direct answer is held as [`Answer::HeadDirect`] with nowhere
//! to keep them, and that difference survives every floor above.

pub mod pane;
pub mod socket;
pub mod state;
pub mod view;

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
mod speech;

pub use pane::{Declined, GrantPane, Principal, ReadPane, SteerPane, settled};
pub use socket::{Admitted, Session};
pub use state::{Answer, Applied, BrowserState, CAUSAL_HEADERS, CausalLink, ChainEnd, ChainWalk};
pub use view::{Axis, Pane};
