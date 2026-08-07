//! Configured prose candidates, live reminder identity, and historical judgments.

#![forbid(unsafe_code)]

mod bag;
mod config;
mod judge;

pub use bag::{
    Bag, BagError, Hit, REMINDER_MARKER, Span, Taste, Trigger, TriggerError, is_reminder_echo,
    normalize_prose, reminder_identity, reminder_text,
};
pub use config::{Config, ConfigError, configured};
pub use judge::{Decision, Judgment, JudgmentFact, judge};
