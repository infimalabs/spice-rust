//! Hermetic repository enforcement.
//!
//! The gate reads a foreign repository and nothing else. It holds no journal
//! and asks none: a verdict is a function of the tree, the policy, and the
//! committed ratchet, so the same three inputs produce the same receipt on any
//! machine and at any hour.
//!
//! [`jurisdiction::Malady`] and [`ratchet::Malady`] are two closed sets under
//! one noun, and they stay module-qualified rather than being flattened here.
//! An ignore line and a ratchet line fail in ways that have nothing to say to
//! each other, and the qualifier is what keeps a reader from believing
//! otherwise.

pub mod input;
pub mod jurisdiction;
pub mod measure;
pub mod message;
pub mod names;
pub mod ratchet;
pub mod verdict;

pub use input::{
    BASELINE_PATH, BASELINE_VERSION, Baseline, BaselineError, Contract, ContractError, FieldError,
    Floor, FloorDeclaration, FloorKind, GateSet, LONG_LINE_LIMIT, POLICY_PATH, PolicyError,
    ToolError, ToolIdentity,
};
pub use jurisdiction::{
    Admission, Entry, Ignore, IgnoreError, Jurisdiction, Rule, Shape, WalkError,
};
pub use measure::{
    Bounds, MeasureError, Measurement, Metric, MetricError, Parser, Policy, measurement_digest,
    render_measurements,
};
pub use message::{CommitMessageError, OriginTrailer, OriginTrailerError, validate_commit_message};
pub use names::{RETIRED_NAMES, RetiredMatch, RetiredName, retired_matches};
pub use ratchet::{Finding, Movement, Ratchet, RatchetError};
pub use verdict::{PublishError, Run, RunError, Standing, Verdict};
