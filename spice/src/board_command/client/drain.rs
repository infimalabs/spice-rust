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
    pub(crate) selection: usize,
    pub(crate) deferred: usize,
    pub(crate) oops: usize,
}

impl ParkedCounts {
    /// Measure possession and set-aside states among authority-filtered
    /// execution rows, and the resident's gate partition among every routed
    /// row. Design-side selection and control therefore remain visible without
    /// inflating execution possession.
    pub(in crate::board_command) fn measure(
        board: &BoardAnswer,
        entries: &[Entry],
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
                .count(),
            workflow_waiting: wait_rows
                .iter()
                .filter(|entry| board.workflow_waiting.contains(entry.born()))
                .count(),
            selection: wait_rows
                .iter()
                .filter(|entry| board.awaiting_selection.contains(entry.born()))
                .count(),
            deferred: execution_rows
                .iter()
                .filter(|entry| entry.workflow().deferral.is_some())
                .count(),
            oops: {
                let satisfied = board::resolved_tasks(entries);
                entries
                    .iter()
                    .filter(|entry| {
                        entry.is_oops()
                            && entry.workflow().status.is_pending()
                            && !satisfied
                                .speculative_design_work(entry.statement(), entry.workflow())
                    })
                    .count()
            },
        }
    }

    /// Pending parked OOPS rows across the whole visible board.
    pub(in crate::board_command) fn oops(entries: &[Entry]) -> usize {
        entries
            .iter()
            .filter(|entry| entry.is_oops() && entry.workflow().status.is_pending())
            .count()
    }

    /// The `board:` line: every non-zero category with its reason, or the
    /// one sentence for a board holding nothing else this lane can see.
    fn board_line(self) -> String {
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
                "following normal Plan or Design workflow",
            ),
            (
                self.selection,
                "selection",
                "awaiting the operator's Design selection",
            ),
            (self.deferred, "deferred", "until their scheduled time"),
            (self.oops, "oops", "parked on the deferred triage board"),
        ]
        .into_iter()
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
    fn new(board: ParkedCounts, team: TeamLife) -> Self {
        let text = [
            "no available tasks".to_owned(),
            board.board_line(),
            format!(
                "team policy: {} ({})",
                team.as_str(),
                team_policy_gloss(team)
            ),
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
        ]
        .join("\n");
        Self { text, board, team }
    }

    pub(super) fn from_next(board: NextWorkCounts, team: TeamLife) -> Self {
        Self::new(
            ParkedCounts {
                active: board.active,
                stale: board.stale,
                blocked: board.blocked,
                workflow_waiting: board.workflow_waiting,
                selection: board.selection,
                deferred: board.deferred,
                oops: board.oops,
            },
            team,
        )
    }
}

/// One `task next` result: the allocator outcome, plus the readout prepared
/// when the allocator drained empty so the rim can speak it for a steered lane.
pub(crate) struct TaskNext {
    pub(crate) outcome: Outcome<CommandAnswer>,
    pub(crate) drain: Option<Reply<DrainReadout>>,
}
