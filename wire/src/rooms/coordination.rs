//! Operator room coordination and put-aside markers, the bounded rule that
//! lets the resident resume a coordination, and the steering controls they
//! drive.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{RoomAnchor, RoomAuthorityStamp};
use crate::SequenceCode;

/// One scoped operator request to coordinate an anchor through native steering.
///
/// The steering fact owns the request text. This fact only links that existing
/// authority into the room timeline, so it never copies conversation bytes or
/// creates another obligation lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCoordinationStarted {
    /// Anchor task; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this room-timeline event.
    pub publication: RoomAuthorityStamp,
    /// Human-sized key of the ordinary steering request.
    pub key: String,
    /// Exact base-52 occupant address asked to make the bounded contribution.
    pub target: crate::OccupantAddress,
    /// Operator rule letting the resident resume this coordination on new
    /// input; every automated record repeats the rule it spends.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub rule: Option<RoomResumeRule>,
    /// Present exactly on a record the resident wrote under `rule`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub automated: Option<RoomAutomatedResume>,
}

/// Most automated resumes one operator rule may authorize.
pub const ROOM_RESUME_LIMIT: u8 = 8;

/// An operator's bounded authority for the resident to resume one room's
/// coordination, to the same target and scope, each time new input arrives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomResumeRule {
    /// Automated resumes authorized, from 1 through [`ROOM_RESUME_LIMIT`].
    pub limit: u8,
    /// Unresolved criterion every resume repeats.
    pub criterion: String,
    /// Bounded contribution every resume repeats.
    pub contribution: String,
    /// Stop condition every resume repeats.
    pub stop: String,
}

/// Provenance of one automated resume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAutomatedResume {
    /// Publication of the operator record whose rule this resume spends.
    pub rule: RoomAuthorityStamp,
    /// This resume's count under that rule, from 1 through its limit.
    pub step: u8,
    /// Room publication by another party that brought the new input.
    pub evidence: RoomAuthorityStamp,
}

/// A live anchor holder asked to checkpoint and release through native task
/// disposition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomStopRequest {
    /// Human-sized key of the ordinary steering request.
    pub key: String,
    /// Exact base-52 address of the holder at admission.
    pub target: crate::OccupantAddress,
    /// Position of the native claim win the request applies to, rendered as
    /// base-52; carry and generation changes keep it.
    pub claim: SequenceCode,
}

/// One operator Put-aside action recorded in an anchor's room timeline.
///
/// The listed keys are ordinary steering dispositions. A stop request remains
/// visible only while the claim win it names remains active.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPutAside {
    /// Anchor task; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this room-timeline event.
    pub publication: RoomAuthorityStamp,
    /// Unanswered room-coordination requests canceled by this action.
    pub canceled: Vec<String>,
    /// Request to the current anchor holder, absent when the anchor was idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<RoomStopRequest>,
}

/// Steering control a room-coordination direction drives.
///
/// The journal binds room coordination records to directions by this exact
/// spelling, so it has one canonical form in each direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoordinationControl {
    /// A scoped request to contribute to the anchor's room.
    Coordinate(RoomAnchor),
    /// A request that the anchor's holder checkpoint and free its claim.
    Stop {
        /// Anchor task whose claim is asked to stop.
        room: RoomAnchor,
        /// Position of the claim win the request applies to.
        claim: SequenceCode,
    },
}

const COORDINATE_CONTROL: &str = "room-coordinate:";
const STOP_CONTROL: &str = "room-stop:";

/// Text that does not spell a room-coordination control.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("not a room coordination control: {0:?}")]
pub struct CoordinationControlError(String);

impl fmt::Display for CoordinationControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Coordinate(room) => write!(formatter, "{COORDINATE_CONTROL}{room}"),
            Self::Stop { room, claim } => write!(formatter, "{STOP_CONTROL}{room}:{claim}"),
        }
    }
}

impl FromStr for CoordinationControl {
    type Err = CoordinationControlError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || CoordinationControlError(text.to_owned());
        if let Some(room) = text.strip_prefix(COORDINATE_CONTROL) {
            return room.parse().map(Self::Coordinate).map_err(|_| invalid());
        }
        let (room, claim) = text
            .strip_prefix(STOP_CONTROL)
            .and_then(|rest| rest.split_once(':'))
            .ok_or_else(invalid)?;
        Ok(Self::Stop {
            room: room.parse().map_err(|_| invalid())?,
            claim: claim.parse().map_err(|_| invalid())?,
        })
    }
}
