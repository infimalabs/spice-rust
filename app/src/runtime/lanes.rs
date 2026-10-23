//! The resident lane topology as the app folds it: teams, their members, and
//! the status a member tile reports.

use std::collections::BTreeMap;

use wire::CanonicalUuid;
use wire::intent::{LaneAnswer, LaneLiveness};

/// Capability-free lane snapshot fields used by the app.
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

    /// Chat team and member composer for one currently admitted lane.
    ///
    /// Independent seats have no team surface, so they intentionally resolve
    /// to no destination.
    #[must_use]
    pub fn chat_target_for_lane(
        &self,
        lane: CanonicalUuid,
    ) -> Option<(CanonicalUuid, CanonicalUuid)> {
        self.lanes
            .iter()
            .find(|team| team.teamed() && team.member(lane).is_some())
            .map(|team| (team.lane, lane))
    }

    /// Chat team and primary composer for one currently admitted team.
    #[must_use]
    pub fn chat_target_for_team(
        &self,
        team: CanonicalUuid,
    ) -> Option<(CanonicalUuid, CanonicalUuid)> {
        let team = self
            .lanes
            .iter()
            .find(|candidate| candidate.lane == team && candidate.teamed())?;
        Some((team.lane, team.members.first()?.lane))
    }
}

/// One app lane: a team and every independently steerable member in it.
#[derive(Clone, PartialEq, Eq)]
pub struct LaneRow {
    /// Stable team identity.
    pub lane: CanonicalUuid,
    /// Authoritative task-allocation lifetime for this team.
    pub subscription: wire::intent::TeamLife,
    /// Whether this team enables fast mode for every member.
    pub fast_mode: bool,
    /// Members in canonical resident seat order.
    pub members: Vec<LaneMember>,
    /// This team's rank by durable formation instant, which decides its base
    /// accent; a lone seat stands in no console.
    pub console: Option<usize>,
}

/// One independently steerable worktree and occupant inside an app lane.
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
    /// Effort this seat pins for its next launch.
    pub seat_effort: Option<String>,
    /// Whether the running occupant runs in fast mode.
    pub fast_mode: bool,
    /// The vendor's words refusing fast mode when that refusal turned it off
    /// on the running occupant, which continues without it.
    pub fast_refusal: Option<String>,
    /// Whether this member's own flag enables fast mode. Its next launch runs
    /// fast when this, its team's flag, or the global setting is on.
    pub seat_fast_mode: bool,
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
    /// This member's status, given how much waking direction is queued for it.
    ///
    /// Pending direction that can wake the lane outranks liveness: a lane
    /// holding unanswered operator steering is the one the operator has to
    /// look at, whether or not anything is running in it. Direction that never
    /// wakes a lane (peer messages, peer replies, resident reminders) waits for
    /// its next seat and leaves the status at liveness. A vacant seat that
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

    /// The accent slot this row deals member `position`. A team's lead wears
    /// its console's base; a lone seat stands in no console and wears the
    /// page's own base, slot zero.
    #[must_use]
    pub const fn accent(&self, position: usize) -> usize {
        match self.console {
            Some(console) => crate::watchband::team_accent(console, position),
            None => crate::watchband::team_accent(0, position),
        }
    }
}

fn deal_team_consoles(
    teams: &mut [LaneRow],
    subscriptions: &BTreeMap<CanonicalUuid, &wire::TeamSubscription>,
) {
    let mut ranked = teams
        .iter()
        .enumerate()
        .filter(|(_, team)| team.teamed())
        .map(|(index, team)| {
            let formed_at = subscriptions[&team.lane]
                .formed_at
                .expect("an admitted team names its formation instant");
            (formed_at, team.lane, index)
        })
        .collect::<Vec<_>>();
    ranked.sort_unstable();
    for (console, (_, _, index)) in ranked.into_iter().enumerate() {
        teams[index].console = Some(console);
    }
}

impl From<&LaneAnswer> for LaneView {
    fn from(answer: &LaneAnswer) -> Self {
        let mut subscriptions = BTreeMap::new();
        for subscription in &answer.subscriptions {
            assert!(
                subscriptions
                    .insert(subscription.team, subscription)
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
                seat_effort: lane.seat_effort.clone(),
                fast_mode: lane.fast_mode,
                fast_refusal: lane.fast_refusal.clone(),
                seat_fast_mode: lane.seat_fast_mode,
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
                let subscription = subscriptions
                    .get(&lane.team)
                    .expect("an admitted lane answer names every team policy");
                teams.push(LaneRow {
                    lane: lane.team,
                    subscription: subscription.policy,
                    fast_mode: subscription.fast_mode,
                    members: vec![member],
                    console: None,
                });
            }
        }
        assert_eq!(
            subscriptions.len(),
            teams.len(),
            "an admitted lane answer names no orphan team policy"
        );
        deal_team_consoles(&mut teams, &subscriptions);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_targets_require_current_teamed_topology() {
        let team = uuid(1);
        let first = uuid(2);
        let second = uuid(3);
        let independent = uuid(4);
        let view = LaneView {
            epoch: uuid(5),
            state: wire::ResidentState::Seated,
            topology_revision: 1,
            lanes: vec![
                LaneRow {
                    lane: team,
                    subscription: wire::intent::TeamLife::Steer,
                    fast_mode: false,
                    members: vec![member(first), member(second)],
                    console: Some(0),
                },
                LaneRow {
                    lane: independent,
                    subscription: wire::intent::TeamLife::Steer,
                    fast_mode: false,
                    members: vec![member(independent)],
                    console: None,
                },
            ],
            refresh_after_millis: None,
            renewals: BTreeMap::new(),
        };

        assert_eq!(view.chat_target_for_lane(second), Some((team, second)));
        assert_eq!(view.chat_target_for_team(team), Some((team, first)));
        assert_eq!(view.chat_target_for_lane(independent), None);
        assert_eq!(view.chat_target_for_team(independent), None);
        assert_eq!(view.chat_target_for_lane(uuid(6)), None);
        assert_eq!(view.chat_target_for_team(uuid(6)), None);
    }

    #[test]
    fn team_console_order_follows_formation_for_every_teamed_row() {
        let older = uuid(1);
        let younger = uuid(2);
        let split = uuid(7);
        let seat = uuid(9);
        let subscription = |team, formed_at: Option<&str>| wire::TeamSubscription {
            team,
            formed_at: formed_at.map(|at| at.parse().expect("fixture formation instant")),
            policy: wire::intent::TeamLife::Drive,
            fast_mode: false,
        };
        let stored = [
            subscription(older, Some("2030-04-12T18:44:09.271Z")),
            subscription(younger, Some("2030-04-12T18:44:10.271Z")),
            subscription(split, Some("2030-04-12T18:44:09.771Z")),
            subscription(seat, None),
        ];
        let subscriptions = stored
            .iter()
            .map(|subscription| (subscription.team, subscription))
            .collect();
        let row = |lane, members: &[CanonicalUuid]| LaneRow {
            lane,
            subscription: wire::intent::TeamLife::Drive,
            fast_mode: false,
            members: members.iter().copied().map(member).collect(),
            console: None,
        };
        let mut teams = [
            row(younger, &[uuid(3), uuid(4)]),
            row(seat, &[seat]),
            row(older, &[uuid(5), uuid(6)]),
            row(split, &[uuid(8)]),
        ];

        deal_team_consoles(&mut teams, &subscriptions);
        assert_eq!(
            teams.iter().map(|team| team.console).collect::<Vec<_>>(),
            [Some(2), None, Some(0), Some(1)],
            "a one-member team under a distinct identity ranks by its formation; a lone seat has no console"
        );
    }

    fn member(lane: CanonicalUuid) -> LaneMember {
        LaneMember {
            lane,
            name: None,
            worktree: format!("/tmp/{lane}"),
            speech: wire::SpeechPlayback::Disabled {},
            life: "live".to_owned(),
            vendor: "omp".parse().expect("fixture vendor"),
            model: None,
            seat_model: None,
            effort: None,
            seat_effort: None,
            fast_mode: false,
            fast_refusal: None,
            seat_fast_mode: false,
            occupant: None,
            session: None,
            disposition: "ready".to_owned(),
            liveness: LaneLiveness::Running,
            bound: true,
            wakeable: true,
            jobs: Vec::new(),
        }
    }

    fn uuid(value: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{value:012}")
            .parse()
            .expect("fixture UUID")
    }
}
