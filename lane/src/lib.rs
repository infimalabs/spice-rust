//! The supervised seat: one agent occupant per lane.
//!
//! A lane is a seat, not a process. The seat outlives the occupant sitting in
//! it, which is why identity, worktree, lineage, and epoch are durable state
//! while the occupant is a process group that comes and goes beneath them.
//! Every module here reduces facts into that durable state or guards a
//! transition of it; none of them decide when to run, because a supervisor that
//! reads a clock cannot be replayed.
//!
//! # Supervisor wake ownership
//!
//! This port permanently owns none of the prototype's nine clock-driven wake
//! arms. They remain predecessor policy, outside this replayable reducer. The
//! complete ownership map is:
//!
//! - `LC-04` (capacity follows work) stays in
//!   `spice/serve/launch.py::AvailableWorkWatch.evaluate` and
//!   `spice/serve/agentapi.py::ensure_agent_for_available_work`, whose serialized
//!   dispatch starts one lane before a later inventory pass can observe more
//!   capacity.
//! - `LC-05` (two-ready-task wake, excluding active claims) stays in
//!   `spice/serve/agentapi.py::ensure_agent_for_available_work` and
//!   `spice/tasks/alloc.py::ordered_visible_ready_rows`.
//! - `LC-06` (lone-task starvation wake) stays in
//!   `spice/serve/agentapi.py::available_work_next_deadline`,
//!   `spice/tasks/readiness.py::queue_ready_epoch`, and
//!   `spice/serve/launch.py::AvailableWorkWatch`.
//! - `LC-07` (burst settle interval) stays in
//!   `spice/serve/agentapi.py::available_work_settle_remaining` and
//!   `ensure_agent_for_available_work`.
//! - `LC-08` (one capacity/select/claim/start decision) stays under
//!   `spice/serve/agentapi.py::_AVAILABLE_WORK_CLAIM_LOCK` in
//!   `ensure_agent_for_available_work` and `_dispatch_available_work_candidate`.
//! - `LC-09` (same-worktree serialization with independent siblings) stays in
//!   `spice/serve/lifecycle.py::LifecycleReconciler._drain_target` and
//!   `LifecycleDecisionAuthority._target_lock`.
//! - `LC-16` (one retry-attempt bucket per lane, consulted only after cheap
//!   declines) stays in
//!   `spice/serve/lifecycle.py::LifecycleDecisionAuthority._attempt_due` and
//!   `_decide_automatic_locked`, with the gate call sites in
//!   `spice/serve/agentapi.py::ensure_agent_for_pending_inbox`,
//!   `ensure_agent_for_held_claim`, and `ensure_agent_for_available_work`.
//! - `LC-17` (refusal names its wake arm) stays in
//!   `spice/serve/agentapi.py::_launch_preflight_failure`, whose callers supply
//!   the `trigger` field.
//! - `LC-18` (shared stop criteria) stays in
//!   `spice/serve/agentapi.py::agent_ensure_response_payload` and
//!   `spice/agent/lifecycle.py::ensure_agent`, including
//!   `_refuse_restart_after_rapid_deaths`; every wake arm reaches that same
//!   ensure path.
//!
//! Moving any arm here would move its wall or monotonic clock with it and turn
//! identical fact streams into different lane states. Rust may receive the
//! resulting deadlines and liveness facts, but it does not schedule their wake.
//!
//! The crate is the widest row in the lattice, sitting above journal, bus,
//! vendor, tape, steer, consent, listen, fold, taste, atomic, and sill. It
//! reaches the plane-crossing vocabulary through [`journal::crossing`] rather
//! than depending on wire directly, so the one vocabulary stays the one every
//! row reads.
//!
//! Time arrives as an argument. [`hold`] compares deadlines it was handed and
//! [`renew`] is told whether an occupant is alive; neither consults a clock, so
//! the same facts read in the same order always reduce to the same seat.

// Accounting is a guarded journal write, and journal gates its whole writer
// vocabulary out of the browser build. Every module reaching that vocabulary
// carries the same gate instead of asking the wasm target to compile a path it
// can never take; reaching only [`journal::crossing`] needs no gate, because
// the crossing types cross every plane by construction.
#[cfg(not(target_arch = "wasm32"))]
pub mod account;
pub mod capture;
pub mod config;
pub mod drain;
pub mod hold;
// Ingestion folds a tape into one accounted batch through [`account`], so it
// holds the writer vocabulary too and follows it behind the same gate.
#[cfg(not(target_arch = "wasm32"))]
pub mod ingest;
pub mod life;
pub mod occupant;
pub mod renew;
pub mod table;

pub use config::Config;

#[cfg(test)]
mod ownership_tests {
    const SOURCE: &str = include_str!("lib.rs");

    #[test]
    fn every_prototype_wake_arm_is_pinned_to_the_predecessor() {
        for number in [4, 5, 6, 7, 8, 9, 16, 17, 18] {
            let marker = format!("`LC-{number:02}`");
            assert_eq!(
                SOURCE.matches(&marker).count(),
                1,
                "{marker} must have exactly one ownership decision"
            );
        }
        assert!(SOURCE.contains("This port permanently owns none"));
        assert!(SOURCE.contains("none of them decide when to run"));
        let forbidden_module = ["pub mod ", "supervisor"].concat();
        assert!(!SOURCE.contains(&forbidden_module));
    }
}
