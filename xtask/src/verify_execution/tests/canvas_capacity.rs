//! Canvas concurrency derived from post-compiler fleet observations.

use super::super::{MAX_CONCURRENT_CARGO_ACTIONS, RESIDENT_CANVAS_SHARD_WORKERS};
use super::*;

#[test]
fn peer_cargo_pressure_continuously_scales_outer_workspace_and_canvas_worker_budgets() {
    let no_pressure = Budget::from_observation(FIXTURE_CPU_COUNT, Some(0));
    let one_peer = Budget::from_observation(FIXTURE_CPU_COUNT, Some(1));
    let two_peers = Budget::from_observation(FIXTURE_CPU_COUNT, Some(2));
    let pressure = Budget::from_observation(FIXTURE_CPU_COUNT, Some(8));
    let one_peer_twelve_cpus = Budget::from_observation(12, Some(1));
    let three_peers = Budget::from_observation(12, Some(3));
    let two_cpus = Budget::from_observation(2, Some(0));
    let one_free_cpu = Budget::from_observation(2, Some(1));
    let saturated_cpu = Budget::from_observation(1, Some(4));
    let unknown = Budget::from_observation(FIXTURE_CPU_COUNT, None);
    assert_eq!(no_pressure.workers, 3);
    assert_eq!(one_peer.workers, 2);
    assert_eq!(pressure.workers, 1);
    assert_eq!(two_cpus.workers, 2);
    assert_eq!(one_free_cpu.workers, 1);
    assert_eq!(saturated_cpu.workers, 1);
    assert_eq!(unknown.workers, 1);
    assert_eq!(unknown.cargo_jobs, 1);
    assert_eq!(no_pressure.cargo_jobs, 8);
    assert_eq!(no_pressure.workspace_workers, 6);
    assert_eq!(one_peer.workspace_workers, 5);
    assert_eq!(two_peers.workspace_workers, 5);
    assert_eq!(pressure.workspace_workers, 1);
    assert_eq!(one_peer_twelve_cpus.cargo_jobs, 4);
    assert_eq!(one_peer_twelve_cpus.workspace_workers, 4);
    assert_eq!(three_peers.cargo_jobs, 3);
    assert_eq!(three_peers.workspace_workers, 3);
    assert_eq!(two_cpus.workspace_workers, 1);
    assert_eq!(one_free_cpu.workspace_workers, 1);
    assert_eq!(saturated_cpu.workspace_workers, 1);
    assert_eq!(unknown.workspace_workers, 1);
    assert_eq!(no_pressure.canvas_workers, 5);
    assert_eq!(one_peer.canvas_workers, 5);
    assert_eq!(two_peers.canvas_workers, 5);
    assert_eq!(pressure.canvas_workers, 5);
    assert_eq!(one_peer_twelve_cpus.canvas_workers, 5);
    assert_eq!(three_peers.canvas_workers, 5);
    assert_eq!(two_cpus.canvas_workers, 4);
    assert_eq!(one_free_cpu.canvas_workers, 4);
    assert_eq!(saturated_cpu.canvas_workers, 4);
    assert_eq!(unknown.canvas_workers, 4);
    for (cpus, active, budget) in [
        (FIXTURE_CPU_COUNT, 0, no_pressure),
        (FIXTURE_CPU_COUNT, 1, one_peer),
        (FIXTURE_CPU_COUNT, 2, two_peers),
        (FIXTURE_CPU_COUNT, 8, pressure),
        (12, 1, one_peer_twelve_cpus),
        (12, 3, three_peers),
    ] {
        if budget.canvas_workers > RESIDENT_CANVAS_SHARD_WORKERS {
            assert!(
                active + budget.workspace_workers + budget.canvas_workers <= cpus,
                "a fifth Canvas lane stays inside the observed CPU fleet"
            );
        }
    }
    assert_eq!(
        no_pressure.cargo_jobs * MAX_CONCURRENT_CARGO_ACTIONS,
        FIXTURE_CPU_COUNT
    );
    assert_eq!(pressure.cargo_jobs, 1);
}
