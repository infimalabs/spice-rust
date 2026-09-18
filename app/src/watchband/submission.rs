//! Where one composer's draft may go, derived once from the connection and the
//! command surface the admitted runtime offers.
//!
//! The surface keeps the last admitted runtime on screen while its connection
//! is down and while a restarted resident hydrates, so the runtime's offers
//! alone describe a resident that may no longer exist. Every submission is
//! witnessed by the occupant and epoch the runtime remembers, and the resident
//! refuses a witness it never seated, so a draft admitted while offline is a
//! draft queued to be refused. Only an online connection turns an offer into
//! authority, and every path that submits a draft asks here.

use wire::{CanonicalUuid, CommandKind};

/// Authority a detached draft may use for its next transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmissionTarget {
    /// Steer the seated occupant under the generation that seated it.
    Resident {
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
    },
    /// Wake the vacant seat and hand it the draft once it is seated.
    Wake,
}

/// What the session may currently do with a composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmissionAuthority {
    /// A seated occupant may be steered.
    pub send: bool,
    /// A vacant bound seat may be woken and then steered.
    pub wake: bool,
}

impl SubmissionAuthority {
    /// Nothing may be submitted.
    pub const NONE: Self = Self {
        send: false,
        wake: false,
    };

    /// Derive from the connection state and the runtime's offers.
    ///
    /// `online` is the surface's connection being `Online`: the socket is
    /// admitted and the runtime it published is the one the resident is
    /// serving now. A wake is a send that first seats an occupant, so it
    /// needs the send offer as well as the lane offers it exercises.
    #[must_use]
    pub fn derive(online: bool, offers: impl Fn(CommandKind) -> bool) -> Self {
        let send = online && offers(CommandKind::SteerSend);
        Self {
            send,
            wake: send && offers(CommandKind::LaneState) && offers(CommandKind::LaneRead),
        }
    }

    /// Where one member's draft goes, or nowhere.
    ///
    /// A seated member is steered when sending is authorised; a vacant member
    /// is woken when waking is and a session was ever declared for the seat.
    /// A member the authority cannot reach has no target, and a draft with no
    /// target stays in its composer.
    #[must_use]
    pub fn target(
        self,
        occupant: Option<CanonicalUuid>,
        bound: bool,
        epoch: CanonicalUuid,
    ) -> Option<SubmissionTarget> {
        match occupant {
            Some(occupant) => self
                .send
                .then_some(SubmissionTarget::Resident { occupant, epoch }),
            None => (self.wake && bound).then_some(SubmissionTarget::Wake),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPERATOR: fn(CommandKind) -> bool = |_| true;

    fn uuid(text: &str) -> CanonicalUuid {
        text.parse().expect("fixture identity is canonical")
    }

    fn occupant() -> CanonicalUuid {
        uuid("33333333-3333-4333-8333-333333333333")
    }

    fn epoch() -> CanonicalUuid {
        uuid("44444444-4444-4444-8444-444444444444")
    }

    #[test]
    fn an_offline_connection_offers_no_authority_whatever_the_runtime_offers() {
        assert_eq!(
            SubmissionAuthority::derive(false, OPERATOR),
            SubmissionAuthority::NONE,
        );
        let offline = SubmissionAuthority::derive(false, OPERATOR);
        assert_eq!(offline.target(Some(occupant()), true, epoch()), None);
        assert_eq!(offline.target(None, true, epoch()), None);
    }

    #[test]
    fn an_online_connection_turns_the_offers_into_authority() {
        let online = SubmissionAuthority::derive(true, OPERATOR);
        assert_eq!(
            online,
            SubmissionAuthority {
                send: true,
                wake: true
            }
        );
        assert_eq!(
            online.target(Some(occupant()), false, epoch()),
            Some(SubmissionTarget::Resident {
                occupant: occupant(),
                epoch: epoch(),
            }),
        );
        assert_eq!(
            online.target(None, true, epoch()),
            Some(SubmissionTarget::Wake)
        );
        assert_eq!(
            online.target(None, false, epoch()),
            None,
            "an unbound seat cannot wake"
        );
    }

    #[test]
    fn a_wake_needs_the_send_offer_and_both_lane_offers() {
        let no_send = SubmissionAuthority::derive(true, |kind| kind != CommandKind::SteerSend);
        assert_eq!(no_send, SubmissionAuthority::NONE);
        let no_state = SubmissionAuthority::derive(true, |kind| kind != CommandKind::LaneState);
        assert_eq!(
            no_state,
            SubmissionAuthority {
                send: true,
                wake: false
            }
        );
        assert_eq!(no_state.target(None, true, epoch()), None);
        let no_read = SubmissionAuthority::derive(true, |kind| kind != CommandKind::LaneRead);
        assert!(!no_read.wake);
    }
}
