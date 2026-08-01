//! Curated prose candidates and the judgments that may turn them into facts.

#![forbid(unsafe_code)]

mod bag;
mod judge;

pub use bag::{Bag, BagError, Hit, Span, Taste, Trigger, TriggerError};
pub use judge::{Decision, Judgment, judge};
