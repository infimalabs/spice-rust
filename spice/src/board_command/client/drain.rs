use serde::Serialize;
use wire::intent::LineageLife;

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
    pub(crate) waiting: usize,
    pub(crate) oops: usize,
}

impl ParkedCounts {
    /// Measure the parked rows among the actor-routed pending rows.
    ///
    /// Blocked asks the board which prerequisites are satisfied so the same
    /// row cannot count ready and blocked at once; parked `.oops` rows are
    /// counted across the whole visible board, as task status does.
    pub(in crate::board_command) fn measure(entries: &[Entry], rows: &[&Entry]) -> Self {
        let satisfied = board::satisfied_tasks(entries);
        let disposition = |entry: &&&Entry, wanted: ClaimDisposition| {
            entry
                .claim()
                .is_some_and(|claim| claim.disposition == wanted)
        };
        Self {
            active: rows
                .iter()
                .filter(|entry| disposition(entry, ClaimDisposition::Active))
                .count(),
            stale: rows
                .iter()
                .filter(|entry| disposition(entry, ClaimDisposition::Lapsed))
                .count(),
            blocked: rows
                .iter()
                .filter(|entry| {
                    entry
                        .statement()
                        .dependencies
                        .iter()
                        .any(|dependency| !satisfied.contains(dependency))
                })
                .count(),
            waiting: rows
                .iter()
                .filter(|entry| entry.workflow().deferral.is_some())
                .count(),
            oops: entries
                .iter()
                .filter(|entry| {
                    entry.project().visibility().hidden_stem() == Some("oops")
                        && entry.workflow().status == "pending"
                        && entry.workflow().completion.is_none()
                })
                .count(),
        }
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
            (self.blocked, "blocked", "waiting on their dependencies"),
            (
                self.waiting,
                "waiting",
                "deferred until their scheduled time",
            ),
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

/// The pending rows the actor route admits, which is what task status counts.
pub(in crate::board_command) fn actor_rows<'a>(
    board: &BoardAnswer,
    entries: &'a [Entry],
) -> Vec<&'a Entry> {
    let routed = board
        .tasks
        .iter()
        .filter(|task| actor_route_matches(board, task))
        .map(|task| task.statement.body.handle.clone())
        .collect::<std::collections::BTreeSet<_>>();
    entries
        .iter()
        .filter(|entry| {
            routed.contains(entry.handle())
                && entry.workflow().status == "pending"
                && entry.workflow().completion.is_none()
        })
        .collect()
}

/// What one lineage policy lets the allocator hand this lane on its own.
///
/// The predecessor had no such gate. Here `Steer` admits no creator lineage
/// at all, so under it every drain is empty by construction, and a lane that
/// is not told so concludes the board is broken.
fn lineage_policy_gloss(policy: LineageLife) -> &'static str {
    match policy {
        LineageLife::Steer => {
            "the operator hands out work explicitly; the allocator selects nothing on its own"
        }
        LineageLife::Drive => "the allocator selects only work created for this lineage",
        LineageLife::Drain => "the allocator selects from all assignable work",
    }
}

/// The typed empty-drain answer a steered lane receives in place of the
/// allocator refusal: the readout text plus the counts and policy it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DrainReadout {
    pub(crate) text: String,
    pub(crate) board: ParkedCounts,
    pub(crate) lineage: LineageLife,
}

impl DrainReadout {
    fn new(board: ParkedCounts, lineage: LineageLife) -> Self {
        let text = [
            "no available tasks".to_owned(),
            board.board_line(),
            format!(
                "lineage policy: {} ({})",
                lineage.as_str(),
                lineage_policy_gloss(lineage)
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
        Self {
            text,
            board,
            lineage,
        }
    }
}

/// Build the empty-drain reply from the board snapshot the allocator drained.
///
/// The counts come from the same snapshot and the same definitions as task
/// status, positioned at the cursor of that snapshot. Rows the actor route
/// rejects and a lane without an allocation subscription are refused exactly
/// as the allocator itself refuses them.
pub(in crate::board_command) fn drain_reply(
    board: &BoardAnswer,
) -> Result<Reply<DrainReadout>, Outcome<CommandAnswer>> {
    let entries = actor_entries(board)?;
    let subscription = allocation_subscription(&board.residency.body, &board.principal)?;
    let rows = actor_rows(board, &entries);
    Ok(Reply {
        position: ReplyPosition::Snapshot(board.cursor),
        body: DrainReadout::new(ParkedCounts::measure(&entries, &rows), subscription.policy),
    })
}

/// One `task next` result: the allocator outcome, plus the readout prepared
/// when the allocator drained empty so the rim can speak it for a steered lane.
pub(crate) struct TaskNext {
    pub(crate) outcome: Outcome<CommandAnswer>,
    pub(crate) drain: Option<Reply<DrainReadout>>,
}
