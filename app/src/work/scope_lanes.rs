//! The lanes working one scope, the twin of a team's task tabs: where a team's
//! tabs are its tasks, a working group's tabs are the lanes working it.

use wire::{Actor, CanonicalUuid};

use super::WorkTask;

/// One lane actively working a scope, with every scoped task it claims.
#[derive(Debug)]
pub struct ScopeLane<'p, 'a> {
    lane: CanonicalUuid,
    tasks: Vec<&'p WorkTask<'a>>,
}

impl<'p, 'a> ScopeLane<'p, 'a> {
    /// Each lane holding a live claim on a task in `strip`, in the order of
    /// its first such task; its tasks keep their strip order. Operator claims
    /// and unclaimed or stale work name no lane.
    #[must_use]
    pub fn working(strip: &[&'p WorkTask<'a>]) -> Vec<Self> {
        let mut lanes = Vec::<Self>::new();
        for &task in strip {
            let Some(Actor::Lane(lane)) = task.working_claimant() else {
                continue;
            };
            if let Some(working) = lanes.iter_mut().find(|working| working.lane == *lane) {
                working.tasks.push(task);
            } else {
                lanes.push(Self {
                    lane: *lane,
                    tasks: vec![task],
                });
            }
        }
        lanes
    }

    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    #[must_use]
    pub fn tasks(&self) -> &[&'p WorkTask<'a>] {
        &self.tasks
    }
}

#[cfg(test)]
mod tests {
    use wire::Actor;
    use wire::head::ClaimDisposition;

    use super::ScopeLane;
    use crate::work::WorkProjection;
    use crate::work::tests::{board, claim, stamp, task};

    #[test]
    fn lanes_follow_their_first_live_claim_and_skip_operator_and_stale_work() {
        let first: wire::CanonicalUuid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            .parse()
            .expect("uuid");
        let second: wire::CanonicalUuid = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
            .parse()
            .expect("uuid");
        let claimed = |value, actor, disposition| {
            let mut row = task(value, "rooms.work");
            let mut head = claim(&row, disposition);
            head.body.actor = actor;
            row.claim = Some(head);
            row
        };
        let board = board(vec![
            claimed(1, Actor::Lane(second), ClaimDisposition::Active),
            claimed(2, Actor::Operator, ClaimDisposition::Active),
            claimed(3, Actor::Lane(first), ClaimDisposition::Active),
            claimed(4, Actor::Lane(second), ClaimDisposition::Active),
            claimed(5, Actor::Lane(first), ClaimDisposition::Freed),
        ]);
        let projection = WorkProjection::from_board(&board);
        let by_stamp = |value| {
            projection
                .tasks()
                .iter()
                .find(|task| task.row().statement.body.born == stamp(value))
                .expect("fixture task")
        };
        let strip = (1..=5).map(by_stamp).collect::<Vec<_>>();

        let shape = ScopeLane::working(&strip)
            .iter()
            .map(|working| {
                let tasks = working
                    .tasks()
                    .iter()
                    .map(|task| task.row().statement.body.born.clone())
                    .collect::<Vec<_>>();
                (working.lane(), tasks)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shape,
            [(second, vec![stamp(1), stamp(4)]), (first, vec![stamp(3)])],
            "one tab per working lane in first-claim order; operator and freed claims name no lane",
        );
    }
}
