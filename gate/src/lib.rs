//! Hermetic repository enforcement.
//!
//! The gate reads a foreign repository and nothing else. It holds no journal
//! and asks none: a verdict is a function of the tree, the policy, and the
//! committed baseline, so the same three inputs produce the same receipt on any
//! machine and at any hour.
//!
//! [`jurisdiction::Malady`] and [`ratchet::Malady`] are two closed sets under
//! one noun, and they stay module-qualified rather than being flattened here.
//! An ignore line and a baseline line fail in ways that have nothing to say to
//! each other, and the qualifier is what keeps a reader from believing
//! otherwise.

pub mod jurisdiction;
pub mod measure;
pub mod message;
pub mod ratchet;
pub mod verdict;

pub use jurisdiction::{
    Admission, Entry, Ignore, IgnoreError, Jurisdiction, Rule, Shape, WalkError,
};
pub use measure::{
    Bounds, MeasureError, Measurement, Metric, MetricError, Parser, Policy, measurement_digest,
    render_measurements,
};
pub use message::{CommitMessageError, OriginTrailer, OriginTrailerError, validate_commit_message};
pub use ratchet::{Baseline, BaselineError, Finding, Movement};
pub use verdict::{PublishError, Run, RunError, Standing, Verdict};
