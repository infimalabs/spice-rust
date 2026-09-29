//! The work one Canvas team owns, in the team Work panel's three lanes.
//!
//! A pure function of one admitted board projection and the team's lane row:
//! no clock and no DOM. Lease expiry never enters it, so a lease renewal
//! cannot move a card.

use std::collections::BTreeSet;

use wire::intent::TeamLife;
use wire::{Actor, CanonicalUuid};

use super::{WorkAvailability, WorkProjection, WorkTask};
use crate::runtime::LaneRow;

/// Why an open team task sits in the Waiting lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamWait {
    /// Requirements or dependencies do not hold.
    Blocked,
    /// An explicit wakeable deferral holds it back.
    Deferred,
    /// Its work awaits a review verdict.
    Review,
}

impl TeamWait {
    /// Stable presentation key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Deferred => "deferred",
            Self::Review => "review",
        }
    }
}

/// One team's open work in disjoint lanes, each ordered by the resident's
/// offer order and then by durable task stamp.
#[derive(Debug, Default)]
pub struct TeamWork<'p, 'a> {
    up_next: Vec<&'p WorkTask<'a>>,
    in_flight: Vec<&'p WorkTask<'a>>,
    waiting: Vec<(TeamWait, &'p WorkTask<'a>)>,
}

impl<'p, 'a> TeamWork<'p, 'a> {
    /// Ready work the team would take next.
    #[must_use]
    pub fn up_next(&self) -> &[&'p WorkTask<'a>] {
        &self.up_next
    }

    /// Work an active claim possesses.
    #[must_use]
    pub fn in_flight(&self) -> &[&'p WorkTask<'a>] {
        &self.in_flight
    }

    /// Work held back, each with the reason it waits.
    #[must_use]
    pub fn waiting(&self) -> &[(TeamWait, &'p WorkTask<'a>)] {
        &self.waiting
    }
}

/// Project the work `team` owns from one admitted board projection.
///
/// An open task belongs to the team when the team created it, a member lane
/// holds its claim or its assignment, or it is in the resident's offer and
/// the team's policy admits its creator team: drain admits every offer, drive
/// only the team's own work, and steer none. Lane precedence is exclusive: an
/// active claim puts a task in flight; otherwise a blocker, a deferral, or
/// the review phase makes it wait; otherwise it is up next. Terminal work
/// belongs to no lane.
#[must_use]
pub fn team_work<'p, 'a>(projection: &'p WorkProjection<'a>, team: &LaneRow) -> TeamWork<'p, 'a> {
    let members = team
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<BTreeSet<_>>();
    let mut work = TeamWork::default();
    for task in projection.tasks() {
        let row = task.row();
        let member_actor = |actor: Option<&Actor>| matches!(actor, Some(Actor::Lane(lane)) if members.contains(lane));
        let owned = row.creator_team == Some(team.lane)
            || member_actor(task.working_claimant())
            || member_actor(row.workflow.body.assignment.as_ref());
        let wait = match task.availability() {
            WorkAvailability::Terminal => continue,
            WorkAvailability::InFlight => {
                if owned {
                    work.in_flight.push(task);
                }
                continue;
            }
            WorkAvailability::Blocked => Some(TeamWait::Blocked),
            WorkAvailability::Deferred => Some(TeamWait::Deferred),
            WorkAvailability::Ready => {
                (row.workflow.body.phase.as_str() == "review").then_some(TeamWait::Review)
            }
        };
        if !owned && !offered_to(task, team.subscription, team.lane) {
            continue;
        }
        match wait {
            Some(wait) => work.waiting.push((wait, task)),
            None => work.up_next.push(task),
        }
    }
    work.up_next.sort_by_key(|task| order(task));
    work.in_flight.sort_by_key(|task| order(task));
    work.waiting.sort_by_key(|(_, task)| order(task));
    work
}

/// Whether the resident offers this task and the team's policy admits it.
fn offered_to(task: &WorkTask<'_>, policy: TeamLife, team: CanonicalUuid) -> bool {
    task.offer_rank().is_some() && policy.admits_creator_team(task.row().creator_team, team)
}

fn order<'t>(task: &WorkTask<'t>) -> (usize, &'t wire::TaskStamp) {
    (
        task.offer_rank().unwrap_or(usize::MAX),
        &task.row().statement.body.born,
    )
}

#[cfg(test)]
mod tests {
    use wire::head::ClaimDisposition;
    use wire::intent::{BoardTask, TeamLife};
    use wire::{Actor, CanonicalUuid, TaskStamp};

    use super::super::tests::{board, claim, stamp, task};
    use super::{TeamWait, TeamWork, team_work};
    use crate::runtime::{BoardView, LaneMember, LaneRow};
    use crate::work::WorkProjection;

    const TEAM: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const OTHER_TEAM: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const MEMBER: &str = "11111111-1111-4111-8111-111111111111";
    const STRANGER: &str = "44444444-4444-4444-8444-444444444444";

    fn uuid(value: &str) -> CanonicalUuid {
        value.parse().expect("fixture uuid")
    }

    fn team(policy: TeamLife) -> LaneRow {
        LaneRow {
            lane: uuid(TEAM),
            subscription: policy,
            members: vec![LaneMember {
                lane: uuid(MEMBER),
                name: None,
                worktree: "/work/member".to_owned(),
                speech: wire::SpeechPlayback::Disabled {},
                life: policy.as_str().to_owned(),
                vendor: "omp".parse().expect("fixture vendor"),
                model: None,
                effort: None,
                occupant: None,
                session: None,
                disposition: "seated".to_owned(),
                liveness: wire::intent::LaneLiveness::Working,
                bound: true,
                wakeable: true,
                jobs: Vec::new(),
            }],
        }
    }

    fn created(value: i64, creator: &str) -> BoardTask {
        let mut row = task(value, "alpha.team");
        row.creator_team = Some(uuid(creator));
        row
    }

    fn claimed_by(mut row: BoardTask, lane: &str) -> BoardTask {
        let mut head = claim(&row, ClaimDisposition::Active);
        head.body.actor = Actor::Lane(uuid(lane));
        row.claim = Some(head);
        row
    }

    fn in_review(mut row: BoardTask) -> BoardTask {
        row.workflow.body.phase = "review".parse().expect("fixture phase");
        row
    }

    fn stamps<'t>(tasks: impl IntoIterator<Item = &'t BoardTask>) -> Vec<TaskStamp> {
        tasks
            .into_iter()
            .map(|row| row.statement.body.born.clone())
            .collect()
    }

    fn lanes(work: &TeamWork<'_, '_>) -> [Vec<TaskStamp>; 3] {
        [
            stamps(work.up_next().iter().map(|task| task.row())),
            stamps(work.in_flight().iter().map(|task| task.row())),
            stamps(work.waiting().iter().map(|(_, task)| task.row())),
        ]
    }

    fn project(board: &BoardView, policy: TeamLife) -> [Vec<TaskStamp>; 3] {
        let projection = WorkProjection::from_board(board);
        lanes(&team_work(&projection, &team(policy)))
    }

    #[test]
    fn each_team_task_lands_in_exactly_one_lane_by_precedence() {
        let mut terminal = claimed_by(created(1, TEAM), MEMBER);
        terminal.workflow.body.completion = Some(
            "2026-09-29T06:00:00.000Z"
                .parse()
                .expect("fixture completion"),
        );
        let mut deferred = created(5, TEAM);
        deferred.workflow.body.deferral = Some(
            "2026-09-30T06:00:00.000Z"
                .parse()
                .expect("fixture deferral"),
        );
        let mut blocked_offer = created(6, TEAM);
        blocked_offer.statement.body.dependencies = vec![stamp(99)];
        let mut assigned_review = in_review(created(7, OTHER_TEAM));
        assigned_review.workflow.body.assignment = Some(Actor::Lane(uuid(MEMBER)));
        let mut board = board(vec![
            terminal,
            claimed_by(created(2, TEAM), STRANGER),
            claimed_by(created(3, OTHER_TEAM), MEMBER),
            claimed_by(in_review(created(4, TEAM)), MEMBER),
            deferred,
            blocked_offer,
            assigned_review,
            created(8, TEAM),
            created(9, OTHER_TEAM),
            claimed_by(created(10, OTHER_TEAM), STRANGER),
        ]);
        board.offer = vec![stamp(6), stamp(8)];

        let projection = WorkProjection::from_board(&board);
        let work = team_work(&projection, &team(TeamLife::Drive));
        assert_eq!(
            lanes(&work),
            [
                vec![stamp(8)],
                vec![stamp(2), stamp(3), stamp(4)],
                vec![stamp(6), stamp(5), stamp(7)],
            ],
            "a claim outranks review, a blocker outranks the offer, and terminal or foreign \
             work belongs to no lane",
        );
        assert_eq!(
            work.waiting()
                .iter()
                .map(|(wait, _)| *wait)
                .collect::<Vec<_>>(),
            [TeamWait::Blocked, TeamWait::Deferred, TeamWait::Review],
        );
    }

    #[test]
    fn only_drain_and_drive_take_the_resident_offer() {
        let mut board = board(vec![
            created(1, OTHER_TEAM),
            created(2, TEAM),
            claimed_by(created(3, OTHER_TEAM), STRANGER),
            task(4, "alpha.team"),
        ]);
        board.offer = vec![stamp(3), stamp(4), stamp(1), stamp(2)];

        let own = [vec![stamp(2)], Vec::new(), Vec::new()];
        assert_eq!(
            project(&board, TeamLife::Steer),
            own,
            "a steer team keeps its own ready work and takes no offer"
        );
        assert_eq!(
            project(&board, TeamLife::Drive),
            own,
            "a drive team takes only offers its own team created"
        );
        assert_eq!(
            project(&board, TeamLife::Drain),
            [vec![stamp(4), stamp(1), stamp(2)], Vec::new(), Vec::new()],
            "a drain team takes every offer not already in flight, in offer order"
        );
    }

    #[test]
    fn lane_order_is_offer_rank_then_stamp_and_ignores_leases() {
        let rows = vec![
            created(4, TEAM),
            created(1, TEAM),
            created(3, TEAM),
            created(2, TEAM),
            claimed_by(created(6, TEAM), MEMBER),
            claimed_by(created(5, TEAM), MEMBER),
        ];
        let mut offered = board(rows);
        offered.offer = vec![stamp(3), stamp(2)];
        let expected = [
            vec![stamp(3), stamp(2), stamp(1), stamp(4)],
            vec![stamp(5), stamp(6)],
            Vec::new(),
        ];
        assert_eq!(project(&offered, TeamLife::Drive), expected);

        let mut renewed = offered.clone();
        for (index, row) in renewed.tasks.iter_mut().enumerate() {
            if let Some(head) = &row.claim {
                let instant = |minute: usize| -> wire::Instant {
                    format!("2026-09-29T06:{minute:02}:00.000Z")
                        .parse()
                        .expect("fixture instant")
                };
                row.lease = Some(wire::intent::Positioned {
                    position: head.position,
                    body: wire::LeaseHead {
                        start: instant(0),
                        renewal: instant(index),
                        maximum: instant(59),
                        expiry: instant(59 - index),
                        epoch: head.body.epoch,
                    },
                });
            }
        }
        assert_eq!(
            project(&renewed, TeamLife::Drive),
            expected,
            "a lease renewal never moves a card"
        );
    }
}
