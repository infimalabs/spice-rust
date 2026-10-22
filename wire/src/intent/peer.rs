//! Peer attribution on a pending directive: who sent it, and which earlier
//! occupant of the receiving lane it named.

use serde::{Deserialize, Serialize};

use crate::ident::{CanonicalUuid, Instant};

/// The exact occupant that authored an MSG peer message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerSender {
    /// Sending occupant, as the message's creating fact records it.
    pub occupant: CanonicalUuid,
    /// That occupant's address, derived from its birth on the sending lane.
    ///
    /// Absent when the journal no longer proves exactly one such birth; an
    /// absent address is reported as missing, never replaced by another.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub address: Option<crate::OccupantAddress>,
}

/// The earlier occupant of a lane that a peer message named, beside the birth
/// of the occupant that heard it instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerAddressed {
    /// The named occupant's address: its birth on the receiving lane.
    pub address: crate::OccupantAddress,
    /// Commit instant of the named occupant's birth.
    pub born: Instant,
    /// Commit instant of the receiving occupant's birth on the same lane.
    pub successor_born: Instant,
}
