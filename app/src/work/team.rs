//! The work one Canvas team holds, for the team Work panel.
//!
//! A pure function of one admitted board projection and the team's lane row:
//! no clock and no DOM. Lease expiry never enters it, so a lease renewal
//! cannot move a card.

use std::collections::BTreeSet;

use super::{WorkAvailability, WorkProjection, WorkTask};
use crate::runtime::LaneRow;

/// The open tasks `team` holds: each has a current active claim by a lane
/// that is a member of `team` now. Offers, team-created work, unclaimed
/// assignments, and blocked, deferred, or review work stay in global Work,
/// so a team view never repeats the global queues. Ordered by the resident's
/// offer order and then by durable task stamp.
#[must_use]
pub fn team_held_work<'p, 'a>(
    projection: &'p WorkProjection<'a>,
    team: &LaneRow,
) -> Vec<&'p WorkTask<'a>> {
    let members = team
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<BTreeSet<_>>();
    let mut held = projection
        .tasks()
        .iter()
        .filter(|task| {
            task.availability() == WorkAvailability::InFlight
                && task
                    .row()
                    .claim
                    .as_ref()
                    .is_some_and(|claim| members.contains(&claim.body.lane))
        })
        .collect::<Vec<_>>();
    held.sort_by_key(|task| order(task));
    held
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
    use super::team_held_work;
    use crate::runtime::{BoardView, LaneMember, LaneRow};
    use crate::work::WorkProjection;

    const TEAM: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const OTHER_TEAM: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const MEMBER: &str = "11111111-1111-4111-8111-111111111111";
    const STRANGER: &str = "44444444-4444-4444-8444-444444444444";

    fn uuid(value: &str) -> CanonicalUuid {
        value.parse().expect("fixture uuid")
    }

    fn team(lane: &str, members: &[&str]) -> LaneRow {
        LaneRow {
            lane: uuid(lane),
            subscription: TeamLife::Drain,
            members: members
                .iter()
                .map(|member| LaneMember {
                    lane: uuid(member),
                    name: None,
                    worktree: "/work/member".to_owned(),
                    speech: wire::SpeechPlayback::Disabled {},
                    life: TeamLife::Drain.as_str().to_owned(),
                    vendor: "omp".parse().expect("fixture vendor"),
                    model: None,
                    seat_model: None,
                    effort: None,
                    seat_effort: None,
                    fast_mode: false,
                    occupant: None,
                    session: None,
                    disposition: "seated".to_owned(),
                    liveness: wire::intent::LaneLiveness::Working,
                    bound: true,
                    wakeable: true,
                    jobs: Vec::new(),
                })
                .collect(),
        }
    }

    fn created(value: i64, creator: &str) -> BoardTask {
        let mut row = task(value, "alpha.team");
        row.creator_team = Some(uuid(creator));
        row
    }

    fn claimed_by(row: BoardTask, lane: &str) -> BoardTask {
        claimed_with(row, lane, ClaimDisposition::Active)
    }

    fn claimed_with(mut row: BoardTask, lane: &str, disposition: ClaimDisposition) -> BoardTask {
        let mut head = claim(&row, disposition);
        head.body.lane = uuid(lane);
        row.claim = Some(head);
        row
    }

    fn in_review(mut row: BoardTask) -> BoardTask {
        row.workflow.body.phase = "review".parse().expect("fixture phase");
        row
    }

    fn held(board: &BoardView, team: &LaneRow) -> Vec<TaskStamp> {
        let projection = WorkProjection::from_board(board);
        team_held_work(&projection, team)
            .iter()
            .map(|task| task.row().statement.body.born.clone())
            .collect()
    }

    #[test]
    fn a_team_holds_only_open_work_its_current_members_actively_claim() {
        let mut deferred = created(3, TEAM);
        deferred.workflow.body.deferral = Some(
            "2026-09-30T06:00:00.000Z"
                .parse()
                .expect("fixture deferral"),
        );
        let mut blocked = created(2, TEAM);
        blocked.statement.body.dependencies = vec![stamp(99)];
        let mut assigned = created(5, OTHER_TEAM);
        assigned.workflow.body.assignment = Some(Actor::Lane(uuid(MEMBER)));
        let mut settled = claimed_by(created(9, TEAM), MEMBER);
        settled.workflow.body.completion = Some(
            "2026-09-29T06:00:00.000Z"
                .parse()
                .expect("fixture completion"),
        );
        let mut board = board(vec![
            created(1, TEAM),
            blocked,
            deferred,
            in_review(created(4, TEAM)),
            assigned,
            claimed_by(created(6, TEAM), STRANGER),
            claimed_by(created(7, OTHER_TEAM), MEMBER),
            claimed_by(in_review(created(8, TEAM)), MEMBER),
            settled,
            claimed_with(created(10, TEAM), MEMBER, ClaimDisposition::Freed),
        ]);
        board.offer = (1..=10).map(stamp).collect();

        assert_eq!(
            held(&board, &team(TEAM, &[MEMBER])),
            [stamp(7), stamp(8)],
            "offers, team-created, assigned, waiting, foreign-claimed, released, and settled \
             work stay out of the team view",
        );
        assert_eq!(
            held(&board, &team(TEAM, &[])),
            Vec::<TaskStamp>::new(),
            "a member who leaves takes its claimed work out of the team",
        );
        assert_eq!(
            held(&board, &team(OTHER_TEAM, &[MEMBER, STRANGER])),
            [stamp(6), stamp(7), stamp(8)],
            "a member's claims follow it into its new team",
        );
    }

    #[test]
    fn held_order_is_offer_rank_then_stamp_and_ignores_leases() {
        let mut offered = board(
            [4, 1, 3, 2]
                .into_iter()
                .map(|value| claimed_by(created(value, TEAM), MEMBER))
                .collect(),
        );
        offered.offer = vec![stamp(3), stamp(2)];
        let expected = [stamp(3), stamp(2), stamp(1), stamp(4)];
        assert_eq!(held(&offered, &team(TEAM, &[MEMBER])), expected);

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
            held(&renewed, &team(TEAM, &[MEMBER])),
            expected,
            "a lease renewal never moves a card"
        );
    }
}
