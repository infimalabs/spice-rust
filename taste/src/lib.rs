//! Configured prose candidates and live reminder identity.
//!
//! One law is executable: a configured bag hit in authored prose becomes one
//! reminder directive. Adjudication is not an arm of it. The historical
//! `taste::confirmed` and `taste::cleared` facts stay decodable as curation
//! evidence, but this crate offers no door that mints another one:
//!
//! ```compile_fail
//! let _ = taste::judge;
//! ```

#![forbid(unsafe_code)]

mod bag;
mod config;

pub use bag::{
    Bag, BagError, Hit, REMINDER_MARKER, Span, Taste, Trigger, TriggerError, is_reminder_echo,
    normalize_prose, reminder_identity, reminder_text,
};
pub use config::{Config, ConfigError, configured};
