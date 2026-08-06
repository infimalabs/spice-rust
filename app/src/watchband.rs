//! The band of lane standings that sits above the feed.
//!
//! One lane's standing is spread across three answers the resident sends
//! separately: the lane table says where it runs and what vendor drives it,
//! the inbox says what direction is waiting for it, and the board says which
//! task it currently holds and how long it has held it. This module performs
//! that join and nothing else, so the arithmetic behind "seated 4m, 2 pending,
//! MOSAIC-1kKQXgDd, review" is reachable from an ordinary native test rather
//! than only from a browser.
//!
//! Every field here is carried by an answer. Nothing is a friendly stand-in
//! the app invented: a lane is named by the identity the resident gave it, and
//! elapsed time comes from the lease, which is the only clock the board hands
//! a task. A field the reference design shows and the wire does not carry is
//! therefore absent here rather than approximated.

use wire::CanonicalUuid;
use wire::head::ClaimDisposition;
use wire::intent::{BoardTask, LaneInbox};

use crate::runtime::LaneRow;

/// Accent slots the band cycles through.
///
/// Keyed to lane identity rather than to row position, so a lane keeps its
/// colour when the table reorders under it and the operator can follow one
/// agent across a redraw by colour alone.
pub const ACCENTS: usize = 6;

/// Characters of a lane identity the band shows.
pub const SHORT_LEN: usize = 8;

/// Characters a band card allows one field.
///
/// A card is one column of a band that has to fit a full lineage across the
/// screen, so the width is the band's property rather than the renderer's.
pub const FIELD_BUDGET: usize = 22;

/// One lane's complete standing, joined from the three answers that carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// Independently steerable member the row describes.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane contains the member.
    pub lineage_id: CanonicalUuid,
    /// Lane identity abbreviated to what a card has room for.
    pub name: String,
    /// Worktree the lane runs in.
    pub worktree: String,
    /// Configured agent name, or the explicit worktree-leaf presentation rule.
    pub label: String,
    /// Vendor driving the lane.
    pub vendor: String,
    /// Concrete model supplied to the occupant's vendor launch.
    pub model: Option<String>,
    /// Reasoning effort supplied to the occupant's vendor launch.
    pub effort: Option<String>,
    /// Current supervised occupant, which is the Canvas agent thread.
    pub thread: Option<CanonicalUuid>,
    /// Exact resumable vendor session identity.
    pub session: Option<String>,
    /// Lifecycle the lane was seated under.
    pub life: String,
    /// Current lane disposition.
    pub disposition: String,
    /// Current supervised live/quiet projection.
    pub liveness: wire::intent::LaneLiveness,
    /// Directives waiting for the lane.
    pub pending: usize,
    /// Producer-supplied preview of the newest waiting directive.
    pub headline: Option<String>,
    /// Handle of the task the lane currently holds.
    pub handle: Option<String>,
    /// Phase that task currently sits in.
    pub phase: Option<String>,
    /// Milliseconds since the claim's lease began.
    pub held: Option<i64>,
    /// Milliseconds since the lease was last renewed.
    pub renewed: Option<i64>,
    /// Accent slot this lane keeps across redraws.
    pub accent: usize,
}

/// Join the lane table, the inbox and the board into one row per lane.
///
/// Lane order is the resident's; this never sorts. `now` is milliseconds since
/// the epoch, passed in rather than read, so the elapsed arithmetic is the same
/// function in a test as in a browser.
#[must_use]
pub fn standings(
    lanes: &[LaneRow],
    inbox: &[LaneInbox],
    tasks: &[BoardTask],
    now: i64,
) -> Vec<Standing> {
    lanes
        .iter()
        .flat_map(|lineage| lineage.members.iter().map(move |member| (lineage, member)))
        .map(|(lineage, member)| {
            let waiting = inbox.iter().find(|row| row.lane == member.lane);
            let held = held_task(tasks, member.lane);
            Standing {
                lane: member.lane,
                lineage_id: lineage.lane,
                name: short(member.lane),
                worktree: member.worktree.clone(),
                label: member
                    .name
                    .clone()
                    .unwrap_or_else(|| leaf(&member.worktree).to_owned()),
                vendor: member.vendor.to_string(),
                model: member.model.clone(),
                effort: member.effort.clone(),
                thread: member.occupant,
                session: member.session.clone(),
                life: member.life.clone(),
                disposition: member.disposition.clone(),
                liveness: member.liveness,
                pending: waiting.map_or(0, |row| row.pending.len()),
                headline: waiting
                    .and_then(|row| row.pending.last())
                    .and_then(|directive| directive.preview.clone()),
                handle: held.map(|task| task.statement.body.handle.to_string()),
                phase: held.map(|task| task.workflow.body.phase.to_string()),
                held: held.and_then(|task| since(task, now, |lease| lease.start)),
                renewed: held.and_then(|task| since(task, now, |lease| lease.renewal)),
                accent: accent(member.lane),
            }
        })
        .collect()
}

/// The task a lane currently holds, or none.
///
/// The join is on the claim's lane rather than on its occupant: both are
/// recorded, but a lane outlives the occupants seated in it, so the lane is
/// the edge that still resolves across a reseat. A lane that has held several
/// tasks carries several claims, so the most recent win — the greatest fact
/// position — is the one showing, and claims the actor already released or let
/// lapse are not standings at all.
fn held_task(tasks: &[BoardTask], lane: CanonicalUuid) -> Option<&BoardTask> {
    tasks
        .iter()
        .filter(|task| {
            task.claim.as_ref().is_some_and(|claim| {
                claim.body.lane == lane && claim.body.disposition == ClaimDisposition::Active
            })
        })
        .max_by_key(|task| task.claim.as_ref().map(|claim| claim.body.fact))
}

/// Milliseconds between one of a task's lease instants and now.
fn since(
    task: &BoardTask,
    now: i64,
    pick: impl Fn(&wire::LeaseHead) -> wire::Instant,
) -> Option<i64> {
    task.lease
        .as_ref()
        .map(|lease| now - pick(&lease.body).get().as_millisecond())
}

/// The accent slot an identity keeps.
///
/// The identity's own low bits, which a version-4 lane fills with randomness,
/// so the slots spread without the app hashing anything. Hashing the canonical
/// spelling instead would fold on the structure lane identities share: the
/// fixed version and variant nibbles sit at fixed offsets, and a digest
/// narrowed to six slots clusters the self-similar spellings a test writes.
#[must_use]
pub fn accent(id: CanonicalUuid) -> usize {
    usize::try_from(id.get().as_u128() % ACCENTS as u128).unwrap_or(0)
}

/// The part of a worktree path that names one lane rather than the fleet.
///
/// Lanes of one fleet are siblings in a single parent directory, so a path cut
/// to what a card has room for keeps only the shared prefix and every card
/// reads alike. The last component is the part that differs, and it is also
/// what an operator calls the lane.
#[must_use]
pub fn leaf(worktree: &str) -> &str {
    worktree
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(worktree)
}

/// An identity abbreviated to what a card has room for.
#[must_use]
pub fn short(id: CanonicalUuid) -> String {
    id.to_string().chars().take(SHORT_LEN).collect()
}

/// A duration in the band's spelling: one unit, no decimals, largest that fits.
///
/// A negative span reads as `now` rather than as a signed number: the only way
/// to reach one is a lease instant fractionally ahead of the browser's clock,
/// and "-0s" would read as a defect rather than as the rounding it is.
#[must_use]
pub fn elapsed_label(millis: i64) -> String {
    let seconds = millis / 1_000;
    match seconds {
        ..=0 => "now".to_owned(),
        1..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// The count a navigation pill wears, or nothing.
///
/// Zero is absence rather than a badge reading `0`: the badge exists to draw
/// the eye, and one that is always present stops doing that.
#[must_use]
pub fn badge(count: usize) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// Fold authored text onto one line, spelling the breaks it had.
///
/// The ticker and the band cards are one line tall, so a newline would
/// otherwise be silently dropped and two sentences would run together as one.
#[must_use]
pub fn one_line(text: &str) -> String {
    text.replace('\r', "")
        .replace('\n', "\\n")
        .replace('\t', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One command outcome as the ticker shows it.
///
/// The class leads because it is the fixed-width part the eye scans down; the
/// detail follows and is what gets cut. Truncation spends one character of the
/// budget on the ellipsis, so the rendered line never exceeds `budget`.
#[must_use]
pub fn ticker_line(class: &str, detail: &str, budget: usize) -> String {
    let line = if detail.is_empty() {
        class.to_owned()
    } else {
        format!("{class}: {}", one_line(detail))
    };
    truncate(&line, budget)
}

/// `text` cut to `budget` characters, ellipsis included in the budget.
#[must_use]
pub fn truncate(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_owned();
    }
    let kept = budget.saturating_sub(1);
    text.chars()
        .take(kept)
        .chain(std::iter::once('…'))
        .collect()
}
