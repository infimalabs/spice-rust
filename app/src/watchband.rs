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

use wire::head::ClaimDisposition;
use wire::intent::{BoardTask, LaneInbox};
use wire::{CanonicalUuid, TaskHandle};

use crate::runtime::LaneRow;

/// Accent slots the band cycles through.
///
/// Slots are dealt in canonical lineage-member order, matching the predecessor:
/// green first, then blue, gold, red, teal, and plum. Identity remains visible
/// in labels and data attributes rather than being inferred from color.
pub const ACCENTS: usize = 6;

/// Largest member-count weight the team console assigns to layout.
///
/// Membership itself is unbounded by presentation. Above this ceiling a team
/// keeps the widest admitted console instead of falling through CSS to a
/// narrower default.
pub const MAX_LINEAGE_VISUAL_WEIGHT: usize = 6;

/// Visual flex/grid weight for one lineage's member count.
#[must_use]
pub fn lineage_visual_weight(members: usize) -> usize {
    members.clamp(1, MAX_LINEAGE_VISUAL_WEIGHT)
}

/// Characters of a lane identity the band shows.
pub const SHORT_LEN: usize = 8;

/// Characters a band card allows one field.
///
/// A card is one column of a band that has to fit a full lineage across the
/// screen, so the width is the band's property rather than the renderer's.
pub const FIELD_BUDGET: usize = 22;

/// Render state for one independently admitted section of the team Tasks view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamTaskSectionState {
    TaskSnapshotUnavailable,
    NoActiveTasks,
    ActiveTasks,
    DirectionSnapshotUnavailable,
    NoQueuedDirection,
    QueuedDirection,
}

impl TeamTaskSectionState {
    /// State of the board-backed active-task section.
    #[must_use]
    pub const fn active(board_available: bool, count: usize) -> Self {
        if !board_available {
            Self::TaskSnapshotUnavailable
        } else if count == 0 {
            Self::NoActiveTasks
        } else {
            Self::ActiveTasks
        }
    }

    /// State of the inbox-backed direction section.
    #[must_use]
    pub const fn direction(inbox_available: bool, count: usize) -> Self {
        if !inbox_available {
            Self::DirectionSnapshotUnavailable
        } else if count == 0 {
            Self::NoQueuedDirection
        } else {
            Self::QueuedDirection
        }
    }

    /// Exact empty or unavailable copy, absent when rows are ready.
    #[must_use]
    pub const fn message(self) -> Option<&'static str> {
        match self {
            Self::TaskSnapshotUnavailable => Some("Task snapshot unavailable."),
            Self::NoActiveTasks => Some("No active tasks for this team."),
            Self::DirectionSnapshotUnavailable => Some("Direction snapshot unavailable."),
            Self::NoQueuedDirection => Some("No queued direction."),
            Self::ActiveTasks | Self::QueuedDirection => None,
        }
    }

    /// Whether assistive technology should announce a missing projection.
    #[must_use]
    pub const fn unavailable(self) -> bool {
        matches!(
            self,
            Self::TaskSnapshotUnavailable | Self::DirectionSnapshotUnavailable
        )
    }
}

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
    /// Current supervised live/quiet fold.
    pub liveness: wire::intent::LaneLiveness,
    /// Directives waiting for the lane.
    pub pending: usize,
    /// Producer-supplied preview of the newest waiting directive.
    pub headline: Option<String>,
    /// Handle of the task the lane currently holds.
    pub handle: Option<String>,
    /// Actor-facing title of the task the lane currently holds.
    pub task_title: Option<String>,
    /// Phase that task currently sits in.
    pub phase: Option<String>,
    /// Epoch milliseconds at which the claim's lease began.
    ///
    /// An instant rather than an elapsed span, because a span is only true for
    /// the frame that computed it. These rows survive across renders while the
    /// resident is quiet, so a stored span would age into a lie; the reader
    /// subtracts a live clock from this instead.
    pub held_since: Option<i64>,
    /// Epoch milliseconds at which the lease was last renewed.
    pub renewed_at: Option<i64>,
    /// Accent slot this lane keeps across redraws.
    pub accent: usize,
}

/// Complete vendor and session detail shown by a standing's hover title.
///
/// A launch recorded before model and effort became durable names that state
/// explicitly. It does not borrow the vendor, session, or another lane as a
/// plausible-looking substitute.
#[must_use]
pub fn standing_title(standing: &Standing) -> String {
    let vendor = match standing.vendor.as_str() {
        "claude" => "Claude vendor",
        "codex" => "Codex vendor",
        "openai" => "OpenAI vendor",
        _ => "Agent vendor",
    };
    let mut parts = vec![vendor.to_owned()];
    parts.push(format!(
        "model: {}",
        standing.model.as_deref().unwrap_or("unrecorded")
    ));
    parts.push(format!(
        "effort: {}",
        standing.effort.as_deref().unwrap_or("unrecorded")
    ));
    parts.push(format!(
        "thread: {}",
        standing
            .thread
            .map_or_else(|| "unbound".to_owned(), |thread| thread.to_string())
    ));
    if let Some(session) = &standing.session {
        parts.push(format!("session: {session}"));
    }
    parts.join("; ")
}

/// Browser composer emblem class for a vendor with a known visual identity.
#[must_use]
pub fn composer_vendor_icon_class(vendor: &str) -> Option<&'static str> {
    match vendor {
        "claude" => Some("composer-vendor-icon--claude"),
        "codex" | "openai" => Some("composer-vendor-icon--openai"),
        _ => None,
    }
}

/// The fraction of a team's width, at either edge, that joins rather than
/// reorders.
///
/// The predecessor's lane drag reads the same gutter: the outer fifth of the
/// team under the pointer fuses the two teams, and the middle three fifths
/// moves the dragged team into that position. Keeping the fraction identical
/// keeps the muscle memory identical, which is the whole reason a number this
/// arbitrary is stated once rather than chosen twice.
pub const LINEAGE_JOIN_GUTTER: f64 = 0.2;

/// What a drop on a team states, decided by where in that team it landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineageDrop {
    /// Insert the dragged team immediately before the target team.
    ///
    /// Stated as "before the target" rather than "into the target's position"
    /// because those coincide only when the dragged team came from later in
    /// the row. Dragging one forward lands it one place ahead of where the
    /// target stood, since removing it first shifts the target back.
    Reorder,
    /// Give the dragged team's agents to the target team.
    Join,
}

impl LineageDrop {
    /// Read the gesture out of a drop's position across the target team.
    ///
    /// `offset` is the pointer's distance from the target's left edge and
    /// `width` the target's width, both in the same units; a target with no
    /// width has no gutter to divide, so it can only reorder.
    #[must_use]
    pub fn at(offset: f64, width: f64) -> Self {
        if width <= 0.0 {
            return Self::Reorder;
        }
        let fraction = offset / width;
        if fraction <= LINEAGE_JOIN_GUTTER || fraction >= 1.0 - LINEAGE_JOIN_GUTTER {
            Self::Join
        } else {
            Self::Reorder
        }
    }
}

/// Every team and the agents it holds, in the order the surface drew them.
///
/// This is the same grouping the canvas renders from, read back out as bare
/// identities: a drag has to answer with the arrangement it *saw*, so the
/// order here is the one the operator was looking at rather than a fresh one.
#[must_use]
pub fn topology(lanes: &[LaneRow]) -> Vec<(CanonicalUuid, Vec<CanonicalUuid>)> {
    lanes
        .iter()
        .map(|lineage| {
            (
                lineage.lane,
                lineage
                    .members
                    .iter()
                    .map(|member| member.lane)
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// State the whole team arrangement one drop produces.
///
/// The resident admits an arrangement only as a complete restatement of the
/// lane table, so this returns every team and every agent rather than the
/// delta: the operator dragged one team, but what the canvas knows and the
/// resident must agree on is where all of them now stand.
///
/// A join appends the dragged team's agents *after* the target's, never
/// before. The target's first agent is its primary composer, so prepending
/// would silently hand the composer to whichever team was dragged in.
///
/// A drop that states nothing returns `None` rather than a no-op command, and
/// the arrangement it would have stated is what decides that. Dropping a team
/// on itself and dropping one that is no longer on the surface are the cases
/// that can be recognised up front; a team dropped on the neighbour it already
/// sits in front of is not, because that gesture is a real drag of a live team
/// onto a different live team and only the result gives it away. Every one of
/// them would otherwise commit a complete restatement of the table the canvas
/// is already showing, writing a durable fact that records no change.
#[must_use]
pub fn arrangement(
    lineages: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    dragged: CanonicalUuid,
    target: CanonicalUuid,
    drop: LineageDrop,
) -> Option<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>> {
    if dragged == target {
        return None;
    }
    let mut arranged = lineages.to_vec();
    let dragged_at = arranged
        .iter()
        .position(|(lineage, _)| *lineage == dragged)?;
    if !arranged.iter().any(|(lineage, _)| *lineage == target) {
        return None;
    }
    let (_, moved) = arranged.remove(dragged_at);
    let target_at = arranged
        .iter()
        .position(|(lineage, _)| *lineage == target)
        .expect("the target survives removing a different team");
    match drop {
        LineageDrop::Reorder => arranged.insert(target_at, (dragged, moved)),
        LineageDrop::Join => arranged[target_at].1.extend(moved),
    }
    (arranged != lineages).then_some(arranged)
}

/// Replace one multi-agent team with one team per agent.
///
/// The first agent keeps the source team's identity so browser-local state can
/// offer a one-click restore after the resident returns the split topology.
/// Every later agent becomes an individual team under its own stable lane
/// identity. Teams outside the target keep both their identity and order.
#[must_use]
pub fn split_lineage(
    lineages: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    target: CanonicalUuid,
) -> Option<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>> {
    let members = lineages
        .iter()
        .find(|(lineage, _)| *lineage == target)?
        .1
        .as_slice();
    if members.len() < 2 {
        return None;
    }
    let mut split = Vec::with_capacity(lineages.len() + members.len() - 1);
    for (lineage, lanes) in lineages {
        if *lineage != target {
            split.push((*lineage, lanes.clone()));
            continue;
        }
        split.extend(
            lanes
                .iter()
                .enumerate()
                .map(|(index, member)| (if index == 0 { target } else { *member }, vec![*member])),
        );
    }
    Some(split)
}

/// Join the lane table, the inbox and the board into one row per lane.
///
/// Lane order is the resident's; this never sorts. No clock reaches this
/// function: the lease instants are copied through untouched, and whoever
/// renders them subtracts the time then. That is what lets a standing outlive
/// the frame that built it — the join changes only when the resident speaks,
/// while the age advances at its next visible unit boundary.
#[must_use]
pub fn standings(lanes: &[LaneRow], inbox: &[LaneInbox], tasks: &[BoardTask]) -> Vec<Standing> {
    lanes
        .iter()
        .flat_map(|lineage| {
            lineage
                .members
                .iter()
                .enumerate()
                .map(move |(position, member)| (lineage, position, member))
        })
        .map(|(lineage, position, member)| {
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
                task_title: held.map(|task| task.presentation_title().to_owned()),
                phase: held.map(|task| task.workflow.body.phase.to_string()),
                held_since: held.and_then(|task| lease_instant(task, |lease| lease.start)),
                renewed_at: held.and_then(|task| lease_instant(task, |lease| lease.renewal)),
                accent: accent(position),
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

/// One of a task's lease instants, in epoch milliseconds.
fn lease_instant(
    task: &BoardTask,
    pick: impl Fn(&wire::LeaseHead) -> wire::Instant,
) -> Option<i64> {
    task.lease
        .as_ref()
        .map(|lease| pick(&lease.body).get().as_millisecond())
}

/// The accent slot dealt to one canonical lineage-member position.
///
/// The seventh member intentionally returns to green. Color decorates visible
/// attribution; lane identity and accessible labels remain authoritative.
#[must_use]
pub const fn accent(position: usize) -> usize {
    position % ACCENTS
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

/// Canonical, colour-independent attribution for one agent-authored card.
///
/// The immutable author and lane lead so narrow cards retain the identities
/// before truncation. The worktree leaf is the human route operators recognize,
/// while a configured name is deliberately redundant presentation at the tail.
#[must_use]
pub fn agent_message_attribution(
    author: CanonicalUuid,
    lane: CanonicalUuid,
    worktree: &str,
    name: Option<&str>,
) -> String {
    let identity = format!("{}/{} · {}", short(author), short(lane), leaf(worktree));
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(|| identity.clone(), |name| format!("{identity} · {name}"))
}

/// Compact visible identity beside one agent-authored card's actions.
///
/// The admitted configured name wins. Without one, the admitted worktree's
/// basename is the only other valid owner. Returning `None` leaves a violated
/// admission assumption visible to the renderer instead of fabricating a third
/// label from lane or author identity.
#[must_use]
pub fn agent_message_label<'a>(worktree: &'a str, name: Option<&'a str>) -> Option<&'a str> {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| worktree.rsplit('/').find(|part| !part.is_empty()))
}

/// Widest string [`elapsed_label`] can return, counted in characters.
///
/// This is a layout contract, not a hint. A ticking age sits in a slot sized
/// once from this number, so the slot can only stay still if the label can
/// never outgrow it — see `age_labels_never_outgrow_the_slot`.
pub const AGE_LABEL_WIDTH: usize = 4;

/// Days past which the label stops counting and says only that it is more.
const AGE_LABEL_MAX_DAYS: i64 = 99;
const MILLIS_PER_SECOND: i64 = 1_000;
const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_DAY: i64 = 86_400;
const LAST_SECOND_OF_MINUTE: i64 = SECONDS_PER_MINUTE - 1;
const LAST_SECOND_OF_HOUR: i64 = SECONDS_PER_HOUR - 1;
const LAST_SECOND_OF_DAY: i64 = SECONDS_PER_DAY - 1;
const MILLIS_PER_MINUTE: i64 = SECONDS_PER_MINUTE * MILLIS_PER_SECOND;
const MILLIS_PER_HOUR: i64 = SECONDS_PER_HOUR * MILLIS_PER_SECOND;
const MILLIS_PER_DAY: i64 = SECONDS_PER_DAY * MILLIS_PER_SECOND;
const MIN_TIMER_DELAY_MILLIS: i128 = 1;

/// A duration in the band's spelling: one unit, no decimals, largest that fits.
///
/// A negative span reads as `now` rather than as a signed number: the only way
/// to reach one is a lease instant fractionally ahead of the browser's clock,
/// and "-0s" would read as a defect rather than as the rounding it is.
///
/// The day arm saturates at `AGE_LABEL_MAX_DAYS` because it is the one arm
/// with no natural ceiling: minutes roll into hours and hours into days, but
/// days count up unbounded, and a single absurd input — an unset instant reads
/// as tens of thousands of days from the browser's epoch — is enough to widen
/// the label past any fixed slot. Saturating discards nothing an operator
/// needs: every caller already carries the exact instants in its `title`, and
/// past three months the distinction being lost is between "long dead" and
/// "long dead".
#[must_use]
pub fn elapsed_label(millis: i64) -> String {
    let seconds = millis / MILLIS_PER_SECOND;
    match seconds {
        ..=0 => "now".to_owned(),
        1..=LAST_SECOND_OF_MINUTE => format!("{seconds}s"),
        SECONDS_PER_MINUTE..=LAST_SECOND_OF_HOUR => {
            format!("{}m", seconds / SECONDS_PER_MINUTE)
        }
        SECONDS_PER_HOUR..=LAST_SECOND_OF_DAY => {
            format!("{}h", seconds / SECONDS_PER_HOUR)
        }
        _ => match seconds / SECONDS_PER_DAY {
            days @ ..=AGE_LABEL_MAX_DAYS => format!("{days}d"),
            _ => format!("{AGE_LABEL_MAX_DAYS}d+"),
        },
    }
}

/// Delay until [`elapsed_label`] can return a different spelling.
///
/// Old labels use their displayed unit as the timer quantum, so restored
/// history does not wake every age each second. A saturated day label has no
/// later spelling and therefore no deadline. Very distant future instants are
/// capped to the browser timer's `u32` input and reconsidered when that bound
/// expires.
#[must_use]
pub fn elapsed_label_deadline_millis(millis: i64) -> Option<u32> {
    let seconds = millis / MILLIS_PER_SECOND;
    let delay = match seconds {
        ..=0 => i128::from(MILLIS_PER_SECOND.saturating_sub(millis)),
        1..=LAST_SECOND_OF_MINUTE => i128::from(until_next_multiple(millis, MILLIS_PER_SECOND)),
        SECONDS_PER_MINUTE..=LAST_SECOND_OF_HOUR => {
            i128::from(until_next_multiple(millis, MILLIS_PER_MINUTE))
        }
        SECONDS_PER_HOUR..=LAST_SECOND_OF_DAY => {
            i128::from(until_next_multiple(millis, MILLIS_PER_HOUR))
        }
        _ if seconds / SECONDS_PER_DAY <= AGE_LABEL_MAX_DAYS => {
            i128::from(until_next_multiple(millis, MILLIS_PER_DAY))
        }
        _ => return None,
    };
    Some(
        u32::try_from(delay.clamp(MIN_TIMER_DELAY_MILLIS, i128::from(u32::MAX)))
            .expect("delay is clamped to u32"),
    )
}

fn until_next_multiple(millis: i64, quantum: i64) -> i64 {
    quantum - millis.rem_euclid(quantum)
}

/// First authored line in a message after borrowed Markdown quotes are skipped.
///
/// Composer headers are intentionally prose-led rather than identity-led. A
/// reply can begin with several levels of quoted context, so the first raw
/// line is not necessarily something the resident wrote. Trimming before the
/// marker check handles indented quote markers, and returning `None` for an
/// entirely quoted message lets the caller continue into older messages.
#[must_use]
pub fn first_nonquoted_prose(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('>'))
        .map(ToOwned::to_owned)
}

/// The count a navigation pill wears, or nothing.
///
/// Zero is absence rather than a badge reading `0`: the badge exists to draw
/// the eye, and one that is always present stops doing that.
#[must_use]
pub fn badge(count: usize) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// Canonical task handles an authored message reports, in first-report order.
///
/// Markdown punctuation is a boundary rather than part of a handle, so prose
/// may name one in backticks, parentheses, or at the end of a sentence. The
/// The wire handle parser admits the outer stem/key shape, then `born` admits
/// the fixed-width base-52 task stamp. Repeated mentions produce one chip
/// rather than turning emphasis into a count.
#[must_use]
pub fn reported_tasks(text: &str) -> Vec<TaskHandle> {
    let mut tasks = Vec::new();
    for (_, task) in task_mentions(text) {
        if !tasks.contains(&task) {
            tasks.push(task);
        }
    }
    tasks
}

/// Canonical task handles and their byte ranges in authored text.
///
/// The browser uses the ranges to replace the handle where it was written,
/// while [`reported_tasks`] retains the de-duplicated identity view used by
/// non-browser surfaces.
#[must_use]
pub fn task_mentions(text: &str) -> Vec<(std::ops::Range<usize>, TaskHandle)> {
    let mut mentions = Vec::new();
    let mut start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if character.is_ascii_alphanumeric() || character == '-' {
            start.get_or_insert(index);
        } else if let Some(start) = start.take()
            && let Some(task) = canonical_task_handle(&text[start..index])
        {
            mentions.push((start..index, task));
        }
    }
    mentions
}

/// Admit one current task identity from a broadly parseable handle.
pub(crate) fn canonical_task_handle(text: &str) -> Option<TaskHandle> {
    let task = text.parse::<TaskHandle>().ok()?;
    task.born().ok()?;
    Some(task)
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

#[cfg(test)]
mod tests {
    use super::{
        MAX_LINEAGE_VISUAL_WEIGHT, agent_message_attribution, agent_message_label,
        composer_vendor_icon_class, lineage_visual_weight,
    };

    #[test]
    fn composer_icons_admit_only_known_vendor_identities() {
        for vendor in ["claude", "codex", "openai"] {
            assert!(composer_vendor_icon_class(vendor).is_some(), "{vendor}");
        }
        for vendor in ["", "agent", "gemini", "Claude", "open-ai"] {
            assert_eq!(composer_vendor_icon_class(vendor), None, "{vendor}");
        }
    }

    #[test]
    fn unnamed_agent_attribution_still_names_author_lane_and_worktree() {
        let author = "11111111-1111-4111-8111-111111111111"
            .parse()
            .expect("the fixture is a canonical author identity");
        let lane = "22222222-2222-4222-8222-222222222222"
            .parse()
            .expect("the fixture is a canonical lane identity");

        assert_eq!(
            agent_message_attribution(author, lane, "/fleet/one/spice-rust-z/", None),
            "11111111/22222222 · spice-rust-z",
        );
    }

    #[test]
    fn configured_agent_name_is_redundant_to_canonical_attribution() {
        let author = "11111111-1111-4111-8111-111111111111"
            .parse()
            .expect("the fixture is a canonical author identity");
        let lane = "22222222-2222-4222-8222-222222222222"
            .parse()
            .expect("the fixture is a canonical lane identity");

        assert_eq!(
            agent_message_attribution(author, lane, "/fleet/one/spice-rust-z", Some(" Ash "),),
            "11111111/22222222 · spice-rust-z · Ash",
        );
    }

    #[test]
    fn message_label_prefers_the_configured_agent_name() {
        assert_eq!(
            agent_message_label("/fleet/one/spice-rust-z", Some(" Ash ")),
            Some("Ash"),
        );
    }

    #[test]
    fn unnamed_message_label_uses_the_worktree_basename() {
        assert_eq!(
            agent_message_label("/fleet/one/spice-rust-z/", None),
            Some("spice-rust-z"),
        );
    }

    #[test]
    fn message_label_refuses_without_a_name_or_worktree_basename() {
        assert_eq!(agent_message_label("/", Some("  ")), None);
    }

    #[test]
    fn lineage_visual_weight_has_a_nonzero_six_share_ceiling() {
        assert_eq!(lineage_visual_weight(0), 1);
        assert_eq!(lineage_visual_weight(1), 1);
        assert_eq!(lineage_visual_weight(MAX_LINEAGE_VISUAL_WEIGHT), 6);
        assert_eq!(lineage_visual_weight(7), 6);
        assert_eq!(lineage_visual_weight(usize::MAX), 6);
    }
}
