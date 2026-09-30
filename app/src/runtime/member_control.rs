//! Lane commands the operator issues from one Dash member, and what each
//! member shows while one is in flight or after the resident refused it.
//!
//! A member runs at most one such command at a time. Its answer settles the
//! exact request it carries, so a late answer to a command abandoned by a
//! reconnect cannot clear or refuse a newer one.

use std::collections::BTreeMap;

use wire::CanonicalUuid;

use super::CanvasRuntime;

/// One lifecycle, succession, or launch-model change for a seated member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberCommand {
    /// Seat an occupant in the vacant seat.
    Wake,
    /// End the current occupancy and leave the seat vacant.
    Drain,
    /// Remove the vacant, claim-free seat from its team.
    Remove,
    /// Request (`true`) or withdraw (`false`) a fresh successor.
    Renew { requested: bool },
    /// Pin the model the next occupant launches with; absent follows the
    /// vendor's configured model.
    Retune { model: Option<String> },
}

impl MemberCommand {
    /// What the member reads while the command is in flight.
    #[must_use]
    pub const fn progress(&self) -> &'static str {
        match self {
            Self::Wake => "Waking…",
            Self::Drain => "Draining…",
            Self::Remove => "Removing…",
            Self::Renew { requested: true } => "Requesting renewal…",
            Self::Renew { requested: false } => "Cancelling renewal…",
            Self::Retune { .. } => "Changing model…",
        }
    }

    /// The action a refusal names.
    #[must_use]
    pub const fn failure(&self) -> &'static str {
        match self {
            Self::Wake => "Could not wake this agent",
            Self::Drain => "Could not drain this agent",
            Self::Remove => "Could not remove this agent",
            Self::Renew { .. } => "Could not renew this agent",
            Self::Retune { .. } => "Could not change this agent's model",
        }
    }
}

/// One exact member command reservation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRequest {
    pub lane: CanonicalUuid,
    /// Reservation order, distinguishing repeats of the same command.
    pub serial: u64,
    pub command: MemberCommand,
}

/// What one member shows about the commands issued from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberControlState {
    pending: Option<MemberRequest>,
    issue: Option<String>,
}

impl MemberControlState {
    /// The command in flight, until its answer lands.
    #[must_use]
    pub const fn pending(&self) -> Option<&MemberCommand> {
        match &self.pending {
            Some(request) => Some(&request.command),
            None => None,
        }
    }

    /// The last refusal, until another command from this member is issued.
    #[must_use]
    pub fn issue(&self) -> Option<&str> {
        self.issue.as_deref()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct MemberControls {
    members: BTreeMap<CanonicalUuid, MemberControlState>,
    serial: u64,
}

impl CanvasRuntime {
    /// In-flight and refusal state for one member, if it ever issued a command.
    #[must_use]
    pub fn member_control(&self, lane: CanonicalUuid) -> Option<&MemberControlState> {
        self.member_controls.members.get(&lane)
    }

    /// Reserve one command for a member, clearing its previous refusal.
    ///
    /// A member with a command already in flight reserves nothing.
    pub fn begin_member_command(
        &mut self,
        lane: CanonicalUuid,
        command: MemberCommand,
    ) -> Option<MemberRequest> {
        let controls = &mut self.member_controls;
        let state = controls.members.entry(lane).or_default();
        if state.pending.is_some() {
            return None;
        }
        controls.serial = controls.serial.saturating_add(1);
        let request = MemberRequest {
            lane,
            serial: controls.serial,
            command,
        };
        state.pending = Some(request.clone());
        state.issue = None;
        Some(request)
    }

    /// Settle the exact request an answer names: success clears it, and a
    /// refusal keeps its reason on the member.
    pub fn settle_member_command(&mut self, request: &MemberRequest, issue: Option<String>) {
        if let Some(state) = self.member_controls.members.get_mut(&request.lane)
            && state.pending.as_ref() == Some(request)
        {
            state.pending = None;
            state.issue = issue;
        }
    }

    /// A reconnect drops every unanswered command; the lane table re-read
    /// that follows shows whichever of them landed.
    pub(super) fn abandon_member_commands(&mut self) {
        for state in self.member_controls.members.values_mut() {
            state.pending = None;
        }
    }
}
