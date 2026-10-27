use serde::Serialize;
use wire::intent::TeamLife;

use super::*;

// The readout for a lane the allocator had nothing for, largely copied from
// the predecessor (spice-z spice/tasks/render.py drain_complete_lines). The
// allocator can prove only that it has no row for this lane. It cannot prove
// that every instruction in the current operator turn was captured or
// resolved, so the readout keeps operator work explicitly independent from
// allocator draining. It still reports the board rather than pointing at
// task status, because sending an agent to inspect parked rows is the hunt
// this readout exists to end.

/// Rows the allocator will not hand over, in the fixed predecessor order.
///
/// Stale claims are split out rather than folded into the held count, because
/// a lapsed claim is not held by anyone: the allocator takes those over when
/// it can, so one still sitting here lost a race or is hidden. It is also the
/// split task status reports, and the two readouts have to agree, so the
/// status renderer measures through this same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ParkedCounts {
    pub(crate) active: usize,
    pub(crate) stale: usize,
    pub(crate) blocked: usize,
    pub(crate) workflow_waiting: usize,
    pub(crate) designs: wire::rooms::DesignCycleCounts,
    pub(crate) deferred: usize,
}

impl ParkedCounts {
    /// Measure possession and set-aside states among authority-filtered
    /// execution rows, and the resident's gate partition among every routed
    /// row. Each open Design counts once, in the cycle state the resident
    /// judged, never as an execution gate.
    pub(in crate::board_command) fn measure(
        board: &BoardAnswer,
        _entries: &[Entry],
        execution_rows: &[&Entry],
        wait_rows: &[&Entry],
    ) -> Self {
        let disposition = |entry: &&&Entry, wanted: ClaimDisposition| {
            entry
                .claim()
                .is_some_and(|claim| claim.disposition == wanted)
        };
        Self {
            active: execution_rows
                .iter()
                .filter(|entry| disposition(entry, ClaimDisposition::Active))
                .count(),
            stale: execution_rows
                .iter()
                .filter(|entry| disposition(entry, ClaimDisposition::Lapsed))
                .count(),
            blocked: wait_rows
                .iter()
                .filter(|entry| board.blocked.contains(entry.born()))
                .filter(|entry| !board.design_cycles.contains_key(entry.born()))
                .count(),
            workflow_waiting: wait_rows
                .iter()
                .filter(|entry| board.workflow_waiting.contains(entry.born()))
                .filter(|entry| !board.design_cycles.contains_key(entry.born()))
                .count(),
            designs: wait_rows
                .iter()
                .filter_map(|entry| board.design_cycles.get(entry.born()))
                .fold(
                    wire::rooms::DesignCycleCounts::default(),
                    |mut counts, cycle| {
                        counts.observe(*cycle);
                        counts
                    },
                ),
            deferred: execution_rows
                .iter()
                .filter(|entry| entry.workflow().deferral.is_some())
                .count(),
        }
    }

    /// The `board:` line: every non-zero category with its reason, or the
    /// one sentence for a board holding nothing else this lane can see.
    fn board_line(self) -> String {
        let designs = wire::rooms::DesignCycle::ALL
            .map(|cycle| (self.designs.get(cycle), cycle.label(), design_reason(cycle)));
        let parked = [
            (self.active, "active", "held by other lanes"),
            (
                self.stale,
                "stale",
                "lapsed claims whose takeover did not land this pass",
            ),
            (
                self.blocked,
                "blocked",
                "execution dependencies cannot advance",
            ),
            (
                self.workflow_waiting,
                "waiting",
                "following normal Plan workflow or advancing prerequisites",
            ),
        ]
        .into_iter()
        .chain(designs)
        .chain(std::iter::once((
            self.deferred,
            "deferred",
            "until their scheduled time",
        )))
        .filter(|(count, _, _)| *count > 0)
        .map(|(count, label, reason)| format!("{count} {label} ({reason})"))
        .collect::<Vec<_>>();
        if parked.is_empty() {
            "board: nothing else visible to this lane".to_owned()
        } else {
            format!("board: {}", parked.join(", "))
        }
    }
}

/// The parked cycle state an aggregate Design count names in the drain
/// readout. One count can mix unclaimed, held, and deferred Designs, so the
/// reason describes the cycle and never names who acts next; per-task surfaces
/// name the exact next action from the Design's own cycle presentation.
const fn design_reason(cycle: wire::rooms::DesignCycle) -> &'static str {
    match cycle {
        wire::rooms::DesignCycle::GatheringInput => "Designs gathering input for their next pass",
        wire::rooms::DesignCycle::ReadyForProposal => {
            "Designs whose next pass can end in a proposal"
        }
        wire::rooms::DesignCycle::AwaitingDecision => "Designs holding an undecided proposal",
        wire::rooms::DesignCycle::WaitingOnSelectedWork => {
            "Designs waiting for the work they selected to finish"
        }
    }
}

/// The pending filed or parked rows the actor route admits, which is what
/// task status counts. Drafts are not work and are counted apart.
pub(in crate::board_command) fn actor_rows<'a>(
    board: &BoardAnswer,
    entries: &'a [Entry],
) -> Vec<&'a Entry> {
    actor_pending(board, entries)
        .filter(|entry| !entry.is_draft())
        .collect()
}

/// Pending Drafts the actor route admits by default.
pub(in crate::board_command) fn actor_drafts(board: &BoardAnswer, entries: &[Entry]) -> usize {
    actor_pending(board, entries)
        .filter(|entry| entry.is_draft())
        .count()
}

fn actor_pending<'a>(board: &BoardAnswer, entries: &'a [Entry]) -> impl Iterator<Item = &'a Entry> {
    let routed = board
        .tasks
        .iter()
        .filter(|task| actor_route_matches(board, task))
        .map(|task| task.statement.body.handle.clone())
        .collect::<std::collections::BTreeSet<_>>();
    entries.iter().filter(move |entry| {
        routed.contains(entry.handle()) && entry.workflow().status.is_pending()
    })
}

/// What one team policy lets the allocator hand this lane on its own.
///
/// The predecessor had no such gate. Here `Steer` admits no creator team
/// at all, so under it every drain is empty by construction, and a lane that
/// is not told so concludes the board is broken.
fn team_policy_gloss(policy: TeamLife) -> &'static str {
    match policy {
        TeamLife::Steer => {
            "the operator hands out work explicitly; the allocator selects nothing on its own"
        }
        TeamLife::Drive => "the allocator selects only work created for this team",
        TeamLife::Drain => "the allocator selects from all assignable work",
    }
}

/// The typed empty-drain answer a steered lane receives in place of the
/// allocator refusal: the readout text plus the counts and policy it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DrainReadout {
    pub(crate) text: String,
    pub(crate) board: ParkedCounts,
    pub(crate) team: TeamLife,
}

impl DrainReadout {
    fn new(board: ParkedCounts, team: TeamLife, boarded_elsewhere: Option<String>) -> Self {
        let text = [
            "no available tasks".to_owned(),
            board.board_line(),
            format!(
                "team policy: {} ({})",
                team.as_str(),
                team_policy_gloss(team)
            ),
        ]
        .into_iter()
        .chain(boarded_elsewhere)
        .chain([
            concat!(
                "allocator scope: no allocator-selected task is available to this lane. ",
                "Parked rows are expected, not missed work. This result does not decide ",
                "whether the current operator request or turn is complete."
            )
            .to_owned(),
            concat!(
                "operator continuity: continue the current prompt and steering ",
                "independently. Perform immediate directions now; capture durable work ",
                "as a task, then run spice task next again."
            )
            .to_owned(),
            concat!(
                "parked-board boundary: do not hunt the parked board for something to ",
                "unstick or unblock. A quiet board or low pending count is not evidence ",
                "that operator work is complete."
            )
            .to_owned(),
        ])
        .collect::<Vec<_>>()
        .join("\n");
        Self { text, board, team }
    }

    pub(super) fn from_next(
        board: &NextWorkCounts,
        team: TeamLife,
        lanes: &[wire::LaneResidency],
    ) -> Self {
        Self::new(
            ParkedCounts {
                active: board.active,
                stale: board.stale,
                blocked: board.blocked,
                workflow_waiting: board.workflow_waiting,
                designs: board.designs,
                deferred: board.deferred,
            },
            team,
            boarded_elsewhere_line(&board.boarded_elsewhere, lanes),
        )
    }
}

/// Names who owns the work this session boarded, so the lane leaves it to
/// them instead of coordinating.
fn boarded_elsewhere_line(
    boarded: &[wire::intent::BoardedElsewhere],
    lanes: &[wire::LaneResidency],
) -> Option<String> {
    if boarded.is_empty() {
        return None;
    }
    let owners = boarded
        .iter()
        .map(|row| {
            let holder = match &row.holder {
                wire::Actor::Lane(lane) => lanes
                    .iter()
                    .find(|seat| seat.lane == *lane)
                    .and_then(|seat| seat.name.clone())
                    .unwrap_or_else(|| row.holder.to_string()),
                other => other.to_string(),
            };
            format!("{} held by {holder}", row.task)
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "boarded work: {owners}. No action required; the owning and review lanes carry it."
    ))
}

/// One `task next` result: the allocator outcome, plus the readout prepared
/// when the allocator drained empty so the rim can speak it for a steered lane.
pub(crate) struct TaskNext {
    pub(crate) outcome: Outcome<CommandAnswer>,
    pub(crate) drain: Option<Reply<DrainReadout>>,
}
