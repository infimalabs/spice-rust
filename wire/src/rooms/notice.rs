//! Resident records of private background notices about open Designs.
//!
//! Each record is the durable read of who was told what; none is a room
//! publication, so none adds room attention, participation, steering, or wake.

use serde::{Deserialize, Serialize};

use super::DecisionRef;
use crate::{CanonicalUuid, SequenceCode, TaskStamp};

/// Most matched terms one Design awareness record carries.
pub const DESIGN_NOTICE_TERMS: usize = 12;

/// The resident told one lane, once for one claim, that an open Design's terms
/// match the task that claim holds.
///
/// The record is background awareness only: it is no room publication, so it
/// adds no room attention, participation, steering, or wake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignNoticed {
    /// The open Design the lane was told about.
    pub design: TaskStamp,
    /// Logical lane told.
    pub lane: CanonicalUuid,
    /// Occupant whose claim the notice served.
    pub occupant: CanonicalUuid,
    /// Task the lane's claim holds.
    pub task: TaskStamp,
    /// Fact that won that claim; each Design notices a lane once per claim.
    pub claim: SequenceCode,
    /// Matched terms in ascending order, at most [`DESIGN_NOTICE_TERMS`].
    pub terms: Vec<String>,
}

/// The resident handed an active Design's holder the operator's newest return
/// decision as its next input.
///
/// One record consumes that decision: no later claimant is told it again, and
/// only a newer return becomes input. It authorizes and mutates nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignReturnDelivered {
    /// The return decision delivered; its controller is the Design.
    pub decision: DecisionRef,
    /// Logical lane holding the Design.
    pub lane: CanonicalUuid,
    /// Occupant told.
    pub occupant: CanonicalUuid,
    /// Fact that won the claim the delivery served.
    pub claim: SequenceCode,
}
