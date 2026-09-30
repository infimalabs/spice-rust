//! The resident lane topology as Canvas folds it: teams, their members, and
//! the status a member tile reports.

use std::collections::BTreeMap;

use wire::CanonicalUuid;
use wire::intent::{LaneAnswer, LaneLiveness};

/// Capability-free lane snapshot fields used by the canvas.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneView {
    /// Resident generation that owns these rows.
    pub epoch: CanonicalUuid,
    /// Current resident state.
    pub state: wire::ResidentState,
    /// Monotonic authority for these exact team rows and policies.
    pub topology_revision: u64,
    /// Safe current lane rows.
    pub lanes: Vec<LaneRow>,
    /// One-shot resident deadline for re-reading the lane liveness snapshot.
    pub refresh_after_millis: Option<u32>,
    /// Durable renewal state keyed by the independently steerable member.
    renewals: BTreeMap<CanonicalUuid, wire::LaneRenewalRecord>,
}

impl LaneView {
    /// Current durable renewal state for one exact team member.
    #[must_use]
    pub fn renewal(
        &self,
        team: CanonicalUuid,
        lane: CanonicalUuid,
    ) -> Option<&wire::LaneRenewalRecord> {
        self.renewals
            .get(&lane)
            .filter(|renewal| renewal.team == team)
    }

    /// Resolve one independently steerable member anywhere in the topology.
    #[must_use]
    pub fn member(&self, lane: CanonicalUuid) -> Option<&LaneMember> {
        self.lanes.iter().find_map(|row| row.member(lane))
    }
}

/// One Canvas lane: a team and every independently steerable member in it.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneRow {
    /// Stable team identity.
    pub lane: CanonicalUuid,
    /// Authoritative task-allocation lifetime for this team.
    pub subscription: wire::intent::TeamLife,
    /// Members in canonical resident seat order.
    pub members: Vec<LaneMember>,
}

/// One independently steerable worktree and occupant inside a Canvas lane.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneMember {
    pub lane: CanonicalUuid,
    pub name: Option<String>,
    pub worktree: String,
    pub speech: wire::SpeechPlayback,
    pub life: String,
    pub vendor: wire::Name,
    /// Model the running occupant was launched with.
    pub model: Option<String>,
    /// Model this seat pins for its next launch, absent when the seat follows
    /// the vendor's configured model. A retune changes this alone.
    pub seat_model: Option<String>,
    pub effort: Option<String>,
    /// Whether the running occupant was launched in fast mode.
    pub fast_mode: bool,
    pub occupant: Option<CanonicalUuid>,
    pub session: Option<String>,
    pub disposition: String,
    /// Dynamic supervised live/quiet fold supplied by the resident.
    pub liveness: LaneLiveness,
    /// Whether a resumable session was ever declared for this seat.
    pub bound: bool,
    /// Whether the resident can seat an occupant here on an operator wake:
    /// a seat with a declared session resumes it, and a seat that was never
    /// started launches fresh. A started seat that lost its session cannot.
    pub wakeable: bool,
    /// Commands the lane started that it has yet to hear the ending of.
    pub jobs: Vec<wire::intent::LaneJob>,
}

/// What a lane tile says about its agent, ranked the way the operator scans.
///
/// Ported from the predecessor 0.31.0 UI, `spice/serve/static/app.lanes.js`:
/// `targetChoiceStatus` derives it, `targetChoiceStatusLabel` names it, and the
/// `targetChoiceStatusValues` list is the rank. That list carries nine states
/// where this wire now distinguishes five: the resident projects recently
/// heard and quiet supervised occupants as `working` and `running`, while
/// richer vendor phases remain absent. The ladder keeps the predecessor's
/// order and names only states the resident actually reports.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LaneStatus {
    /// Direction is queued for this lane; the operator is being waited on.
    Queued,
    /// An occupant was heard from inside the quiet horizon, so what it is up
    /// to right now is known in detail.
    Working,
    /// An occupant holds the seat under supervision; nothing more is claimed.
    Running,
    /// Nobody is in the seat, but a command it started still runs or ended
    /// unheard, and its next occupant is owed the ending.
    Waiting,
    /// The seat has a declared session but nobody in it.
    Bound,
    /// The seat has never been occupied.
    Unbound,
}

impl LaneStatus {
    /// Stable machine spelling used by browser hooks and status styling.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Working => "working",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Bound => "bound",
            Self::Unbound => "unbound",
        }
    }

    /// The predecessor's exact wording for this state.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "steering queued",
            Self::Working => "agent working",
            Self::Running => "agent running",
            Self::Waiting => "agent waiting",
            Self::Bound => "agent bound",
            Self::Unbound => "agent unbound",
        }
    }
}

impl LaneMember {
    /// This member's status, given how much direction is queued for it.
    ///
    /// Pending direction outranks liveness, exactly as the predecessor ranks
    /// it: a lane holding unanswered steering is the one the operator has to
    /// look at, whether or not anything is running in it. A vacant seat that
    /// still owes its next occupant a command's ending is waiting, not cold.
    #[must_use]
    pub const fn status(&self, pending: usize) -> LaneStatus {
        if pending > 0 {
            LaneStatus::Queued
        } else {
            match self.liveness {
                LaneLiveness::Working => LaneStatus::Working,
                LaneLiveness::Running => LaneStatus::Running,
                LaneLiveness::Vacant if !self.jobs.is_empty() => LaneStatus::Waiting,
                LaneLiveness::Vacant if self.bound => LaneStatus::Bound,
                LaneLiveness::Vacant => LaneStatus::Unbound,
            }
        }
    }
}

impl LaneRow {
    /// The team host and main agent: canonical member zero at the right control edge.
    #[must_use]
    pub fn host(&self) -> Option<&LaneMember> {
        self.members.first()
    }

    /// Resolve one independently steerable member of this team.
    #[must_use]
    pub fn member(&self, lane: CanonicalUuid) -> Option<&LaneMember> {
        self.members.iter().find(|member| member.lane == lane)
    }

    /// Whether this row is a team the fleet renders rather than a lone agent.
    #[must_use]
    pub fn teamed(&self) -> bool {
        wire::head::teamed(self.lane, self.members.iter().map(|member| member.lane))
    }
}

impl From<&LaneAnswer> for LaneView {
    fn from(answer: &LaneAnswer) -> Self {
        let mut subscriptions = BTreeMap::new();
        for subscription in &answer.subscriptions {
            assert!(
                subscriptions
                    .insert(subscription.team, subscription.policy)
                    .is_none(),
                "an admitted lane answer names one policy per team"
            );
        }
        let mut renewals = BTreeMap::new();
        for renewal in &answer.renewals {
            assert!(
                renewals.insert(renewal.lane, renewal.clone()).is_none(),
                "an admitted lane answer names one renewal state per member"
            );
        }
        let mut teams = Vec::<LaneRow>::new();
        for lane in &answer.lanes {
            let member = LaneMember {
                lane: lane.lane,
                name: lane.name.clone(),
                worktree: lane.worktree.clone(),
                speech: lane.speech.clone(),
                life: lane.life.clone(),
                vendor: lane.vendor.clone(),
                model: lane.model.clone(),
                seat_model: lane.seat_model.clone(),
                effort: lane.effort.clone(),
                fast_mode: lane.fast_mode,
                occupant: lane.occupant,
                session: lane.session.clone(),
                disposition: lane.disposition.clone(),
                liveness: lane.liveness,
                bound: lane.bound,
                wakeable: lane.bound || lane.lineage.is_empty(),
                jobs: lane.jobs.clone(),
            };
            if let Some(team) = teams.iter_mut().find(|team| team.lane == lane.team) {
                team.members.push(member);
            } else {
                teams.push(LaneRow {
                    lane: lane.team,
                    subscription: *subscriptions
                        .get(&lane.team)
                        .expect("an admitted lane answer names every team policy"),
                    members: vec![member],
                });
            }
        }
        assert_eq!(
            subscriptions.len(),
            teams.len(),
            "an admitted lane answer names no orphan team policy"
        );
        Self {
            epoch: answer.epoch,
            state: answer.state,
            topology_revision: answer.topology_revision,
            lanes: teams,
            refresh_after_millis: answer.refresh_after_millis,
            renewals,
        }
    }
}
