use super::*;
use crate::work::tests::{board, claim, stamp, task};
use wire::EnvironmentRequirement;
use wire::head::ClaimDisposition;

#[test]
fn only_chains_agents_cannot_move_are_walls() {
    let mut active = task(1, "wall.chain");
    active.claim = Some(claim(&active, ClaimDisposition::Active));
    let ready = task(2, "wall.chain");
    let mut waiting = task(3, "wall.chain");
    waiting.statement.body.dependencies = vec![stamp(1), stamp(2)];
    let mut transitive = task(4, "wall.chain");
    transitive.statement.body.dependencies = vec![stamp(3)];
    let mut deferred = task(5, "wall.chain");
    deferred.workflow.body.deferral = Some("2026-09-24T06:00:00.000Z".parse().expect("deferral"));
    let mut upgrade = task(6, "wall.chain");
    upgrade.statement.body.requirements = wire::EnvironmentRequirements::one(
        EnvironmentRequirement::journal_schema(2).expect("requirement"),
    );
    let mut closed = task(7, "wall.chain");
    closed.workflow.body.status = "closed".to_owned();
    let mut behind_deferral = task(8, "wall.chain");
    behind_deferral.statement.body.dependencies = vec![stamp(5)];
    let mut behind_upgrade = task(9, "wall.chain");
    behind_upgrade.statement.body.dependencies = vec![stamp(4), stamp(6)];
    let mut cycle_a = task(10, "wall.chain");
    cycle_a.statement.body.dependencies = vec![stamp(11)];
    let mut cycle_b = task(11, "wall.chain");
    cycle_b.statement.body.dependencies = vec![stamp(10)];
    let mut closed_or_settled = task(12, "wall.chain");
    closed_or_settled.statement.body.dependencies = vec![stamp(7), stamp(99)];

    let mut board = board(vec![
        active,
        ready,
        waiting,
        transitive,
        deferred,
        upgrade,
        closed,
        behind_deferral,
        behind_upgrade,
        cycle_a,
        cycle_b,
        closed_or_settled,
    ]);
    // The resident's gate holds back every row waiting on an unsettled prerequisite.
    board.blocked.extend([3, 4, 8, 9, 10, 11, 12].map(stamp));
    let work = WorkProjection::from_board(&board);
    let mut walls = DependencyWalls::new(&work);
    let mut walls_of = |index: usize| {
        work.tasks()[index]
            .dependencies()
            .iter()
            .map(|dependency| walls.wall(index, dependency).map(|(_, wall)| wall))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        walls_of(2),
        [None, None],
        "claimed and assignable work moves"
    );
    assert_eq!(walls_of(3), [None], "a chain of waits on moving work moves");
    assert_eq!(walls_of(7), [Some(DependencyWall::Deferred(&stamp(5)))]);
    assert_eq!(
        walls_of(8),
        [None, Some(DependencyWall::Environment(&stamp(6)))],
    );
    assert_eq!(walls_of(9), [Some(DependencyWall::Cycle(&stamp(10)))]);
    assert_eq!(
        walls_of(11),
        [Some(DependencyWall::Unreachable(&stamp(7))), None],
        "a closed prerequisite is a wall; one the open board omits settled",
    );
}
