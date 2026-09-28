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

#[cfg(any(target_arch = "wasm32", test))]
mod composer_drag;
mod renewal;
mod submission;
mod vendor;

#[cfg(target_arch = "wasm32")]
pub(crate) use composer_drag::ComposerDragSession;
pub use renewal::{AgentRenewalControl, agent_renewal_control};
pub use submission::{SubmissionAuthority, SubmissionTarget};
pub use vendor::{OmpEmblem, composer_vendor_icon_class, omp_emblem, omp_provider};

/// Accent slots the band cycles through.
///
/// Slots are dealt from the right control edge, matching the predecessor:
/// green first, then blue, gold, red, teal, and plum moving left. Identity
/// remains visible in labels and data attributes rather than being inferred
/// from color.
pub const ACCENTS: usize = 6;

/// Largest member-count weight the team console assigns to layout.
///
/// Membership itself is unbounded by presentation. Above this ceiling a team
/// keeps the widest admitted console instead of falling through CSS to a
/// narrower default.
pub const MAX_TEAM_VISUAL_WEIGHT: usize = 6;

/// Visual flex/grid weight for one team's member count.
#[must_use]
pub fn team_visual_weight(members: usize) -> usize {
    members.clamp(1, MAX_TEAM_VISUAL_WEIGHT)
}

/// Characters of a lane identity the band shows.
pub const SHORT_LEN: usize = 8;

/// Characters a band card allows one field.
///
/// A card is one column of a band that has to fit a full team across the
/// screen, so the width is the band's property rather than the renderer's.
pub const FIELD_BUDGET: usize = 22;

/// Stable bounded identity when neither configured name nor worktree leaf exists.
pub const UNKNOWN_AGENT_LABEL: &str = "Unknown agent";

/// Render state for one independently admitted section of the team Work view.
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
    /// Canvas team whose lane contains the member.
    pub team: CanonicalUuid,
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
    /// Commands the lane started and has yet to hear the ending of.
    pub jobs: Vec<wire::intent::LaneJob>,
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
    /// Epoch milliseconds at which the current lease expires.
    pub expires_at: Option<i64>,
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
        "omp" => "oh-my-pi vendor",
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
    if let Some(kept) = jobs_kept(&standing.jobs) {
        parts.push(kept);
    }
    parts.join("; ")
}

/// The commands a lane still keeps, as one line: "waiting on 1 job (1
/// running)" or "waiting on 3 jobs (2 running, 1 unheard)". None when the
/// lane keeps nothing, so a caller adds the line only when there is one.
#[must_use]
pub fn jobs_kept(jobs: &[wire::intent::LaneJob]) -> Option<String> {
    if jobs.is_empty() {
        return None;
    }
    let plural = if jobs.len() == 1 { "" } else { "s" };
    let running = jobs
        .iter()
        .filter(|job| job.standing == wire::intent::LaneJobStanding::Running)
        .count();
    let unheard = jobs.len() - running;
    let breakdown = [(running, "running"), (unheard, "unheard")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, standing)| format!("{count} {standing}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "waiting on {} job{plural} ({breakdown})",
        jobs.len()
    ))
}

/// The fraction of a team's width, at either edge, that joins rather than
/// reorders.
///
/// The predecessor's lane drag reads the same gutter: the outer fifth of the
/// team under the pointer fuses the two teams, and the middle three fifths
/// moves the dragged team into that position. Keeping the fraction identical
/// keeps the muscle memory identical, which is the whole reason a number this
/// arbitrary is stated once rather than chosen twice.
pub const TEAM_JOIN_GUTTER: f64 = 0.2;

/// What a drop on a team states, decided by where in that team it landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamDrop {
    /// Insert the dragged team immediately before the target team.
    ///
    /// Stated as "before the target" rather than "into the target's position"
    /// because those coincide only when the dragged team came from later in
    /// the row. Dragging one forward lands it one place ahead of where the
    /// target stood, since removing it first shifts the target back.
    Reorder,
    /// Insert the dragged team immediately after the target team.
    ReorderAfter,
    /// Give the dragged team's agents to the target team.
    Join,
}

impl TeamDrop {
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
        if fraction <= TEAM_JOIN_GUTTER || fraction >= 1.0 - TEAM_JOIN_GUTTER {
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
        .map(|team| {
            (
                team.lane,
                team.members
                    .iter()
                    .map(|member| member.lane)
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// Swap exactly the two composers named by one drag preview.
///
/// The complete topology is returned because team arrangement is an atomic
/// restatement. Team identities, team order, and every unhovered member stay
/// fixed; only the two member slots exchange identities. The message plane
/// never consumes this ordering, so a composer swap cannot reorder cards.
#[must_use]
pub fn swap_members(
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    source: CanonicalUuid,
    target: CanonicalUuid,
) -> Option<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>> {
    if source == target {
        return None;
    }
    let mut source_slot = None;
    let mut target_slot = None;
    for (team_at, (_, members)) in topology.iter().enumerate() {
        for (member_at, member) in members.iter().enumerate() {
            if *member == source {
                source_slot = Some((team_at, member_at));
            } else if *member == target {
                target_slot = Some((team_at, member_at));
            }
        }
    }
    let (source_team, source_at) = source_slot?;
    let (target_team, target_at) = target_slot?;
    if source_team != target_team {
        return None;
    }
    let mut swapped = topology.to_vec();
    swapped[source_team].1.swap(source_at, target_at);
    Some(swapped)
}

/// Move one agent from its team to the end of another team.
///
/// Appending keeps the destination's first agent, its primary composer, in
/// place, exactly as a join does. The source team keeps its identity and the
/// order of its remaining agents; a team the move empties leaves the topology,
/// as a joined team does. A drop on the agent's own team, or on a team no longer
/// on the surface, states nothing.
#[must_use]
pub fn transfer_member(
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    member: CanonicalUuid,
    target: CanonicalUuid,
) -> Option<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>> {
    let source_at = teams
        .iter()
        .position(|(_, lanes)| lanes.contains(&member))?;
    let target_at = teams.iter().position(|(team, _)| *team == target)?;
    if source_at == target_at {
        return None;
    }
    let mut arranged = teams.to_vec();
    arranged[source_at].1.retain(|lane| *lane != member);
    arranged[target_at].1.push(member);
    if arranged[source_at].1.is_empty() {
        arranged.remove(source_at);
    }
    Some(arranged)
}

/// The teams in one topology, in the order the canvas drew them.
///
/// These are the rows the fleet renders and an arrangement can target. A row
/// holding one agent under its own identity is that agent's default seat, not
/// a team: it stays in the topology so a restatement remains complete, but no
/// surface stands for it.
pub fn teams(
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
) -> impl Iterator<Item = CanonicalUuid> + '_ {
    topology
        .iter()
        .filter(|(team, members)| wire::head::teamed(*team, members.iter().copied()))
        .map(|(team, _)| *team)
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
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    dragged: CanonicalUuid,
    target: CanonicalUuid,
    drop: TeamDrop,
) -> Option<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>> {
    if dragged == target {
        return None;
    }
    let mut arranged = teams.to_vec();
    let dragged_at = arranged.iter().position(|(team, _)| *team == dragged)?;
    if !arranged.iter().any(|(team, _)| *team == target) {
        return None;
    }
    let (_, moved) = arranged.remove(dragged_at);
    let target_at = arranged
        .iter()
        .position(|(team, _)| *team == target)
        .expect("the target survives removing a different team");
    match drop {
        TeamDrop::Reorder => arranged.insert(target_at, (dragged, moved)),
        TeamDrop::ReorderAfter => arranged.insert(target_at + 1, (dragged, moved)),
        TeamDrop::Join => arranged[target_at].1.extend(moved),
    }
    (arranged != teams).then_some(arranged)
}

/// Replace one multi-agent team with one team per agent.
///
/// The first agent keeps the source team's identity so browser-local state can
/// offer a one-click restore after the resident returns the split topology.
/// Every later agent becomes an individual team under its own stable lane
/// identity. Orgs outside the target keep both their identity and order.
#[must_use]
pub fn split_team(
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    target: CanonicalUuid,
) -> Option<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>> {
    let members = teams.iter().find(|(team, _)| *team == target)?.1.as_slice();
    if members.len() < 2 {
        return None;
    }
    let mut split = Vec::with_capacity(teams.len() + members.len() - 1);
    for (team, lanes) in teams {
        if *team != target {
            split.push((*team, lanes.clone()));
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

/// Move one member of a multi-agent team into one fresh team.
///
/// The source team keeps its identity and the relative order of every sibling.
/// The fresh team stands immediately after it, matching the operator's local
/// action without disturbing any unrelated team.
#[must_use]
pub fn split_member(
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    source: CanonicalUuid,
    member: CanonicalUuid,
    new_team: CanonicalUuid,
) -> Option<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>> {
    if source == new_team || teams.iter().any(|(team, _)| *team == new_team) {
        return None;
    }
    let mut arranged = teams.to_vec();
    let source_at = arranged.iter().position(|(team, _)| *team == source)?;
    if arranged[source_at].1.len() < 2 {
        return None;
    }
    let member_at = arranged[source_at]
        .1
        .iter()
        .position(|lane| *lane == member)?;
    let member = arranged[source_at].1.remove(member_at);
    arranged.insert(source_at + 1, (new_team, vec![member]));
    Some(arranged)
}

/// Whether one member can leave every team while keeping its lane seat.
#[must_use]
pub fn can_leave_all_teams(
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    source: CanonicalUuid,
    member: CanonicalUuid,
) -> bool {
    teams.iter().any(|(team, members)| {
        *team == source && members.contains(&member) && (source != member || members.len() > 1)
    })
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
        .flat_map(|team| {
            team.members
                .iter()
                .enumerate()
                .map(move |(position, member)| (team, position, member))
        })
        .map(|(team, position, member)| {
            let waiting = inbox.iter().find(|row| row.lane == member.lane);
            let held = held_task(tasks, member.lane);
            Standing {
                lane: member.lane,
                team: team.lane,
                name: short(member.lane),
                worktree: member.worktree.clone(),
                label: agent_message_label(&member.worktree, member.name.as_deref()).to_owned(),
                vendor: member.vendor.to_string(),
                model: member.model.clone(),
                effort: member.effort.clone(),
                thread: member.occupant,
                session: member.session.clone(),
                life: member.life.clone(),
                disposition: member.disposition.clone(),
                liveness: member.liveness,
                jobs: member.jobs.clone(),
                pending: waiting.map_or(0, |row| row.pending.len()),
                headline: waiting
                    .and_then(|row| row.pending.last())
                    .and_then(|directive| directive.preview.clone()),
                handle: held.map(|task| task.statement.body.handle.to_string()),
                task_title: held.map(|task| task.presentation_title().to_owned()),
                phase: held.map(|task| task.workflow.body.phase.to_string()),
                held_since: held.and_then(|task| lease_instant(task, |lease| lease.start)),
                expires_at: held.and_then(|task| lease_instant(task, |lease| lease.expiry)),
                accent: accent(team.members.len(), position),
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

/// The zero-based visual ordinal of one member counted from the right edge.
///
/// Canonical member order is also DOM order, from left to right. The predecessor
/// renders the same order with `row-reverse`, so translating through this
/// ordinal preserves Rust's geometry while keeping its right-handed deal.
#[must_use]
pub const fn right_edge_ordinal(members: usize, position: usize) -> usize {
    assert!(position < members, "member position belongs to its team");
    members - position - 1
}

/// The accent slot dealt to one canonical team-member position.
///
/// The rightmost member is green. Moving left advances through the six slots,
/// and a seventh member intentionally returns to green. Color decorates visible
/// attribution; lane identity and accessible labels remain authoritative.
#[must_use]
pub const fn accent(members: usize, position: usize) -> usize {
    right_edge_ordinal(members, position) % ACCENTS
}

/// Resolve the accent slot currently dealt to one independently steerable lane.
///
/// Agent color follows canonical team position, so every surface that names a
/// lane uses the same right-edge palette deal.
#[must_use]
pub fn lane_accent(lanes: &[LaneRow], lane: CanonicalUuid) -> Option<usize> {
    lanes.iter().find_map(|team| {
        team.members
            .iter()
            .position(|member| member.lane == lane)
            .map(|position| accent(team.members.len(), position))
    })
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

/// The words an image tool card keeps for its image.
///
/// A tool card shows its image alone, so the resident's preview line -- the
/// call and its outcome, `read: /tmp/shots/emblem.png succeeded` -- lives on
/// as the image's description instead of as visible text. Directories are the
/// part a reader cannot use there, so every path is cut to its base name:
/// `read: emblem.png succeeded`. A path is a run of characters a shell or JSON
/// argument leaves unquoted, so `view_image("/tmp/shots/emblem.png")` keeps
/// its call and its quotes around `emblem.png`.
#[must_use]
pub fn tool_media_description(preview: &str) -> String {
    preview
        .split_inclusive(bounds_path)
        .map(|piece| {
            let run = piece.trim_end_matches(bounds_path);
            if run.contains('/') {
                format!("{}{}", leaf(run), &piece[run.len()..])
            } else {
                piece.to_owned()
            }
        })
        .collect()
}

/// The punctuation a shell or JSON argument wraps a path in.
const PATH_BOUNDS: &str = "\"'`()[]{}<>,;=|&";

/// Whitespace and punctuation end a path inside a preview line.
fn bounds_path(character: char) -> bool {
    character.is_whitespace() || PATH_BOUNDS.contains(character)
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
/// basename is the only other identified owner. When neither exists, one fixed
/// bounded label keeps the missing identity visible without exposing lane IDs.
#[must_use]
pub fn agent_message_label<'a>(worktree: &'a str, name: Option<&'a str>) -> &'a str {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| worktree.rsplit('/').find(|part| !part.is_empty()))
        .unwrap_or(UNKNOWN_AGENT_LABEL)
}

#[cfg(target_arch = "wasm32")]
pub(crate) const fn agent_activity_name(activity: wire::AgentActivityKind) -> &'static str {
    match activity {
        wire::AgentActivityKind::Prose => "prose",
        wire::AgentActivityKind::Tool => "tool",
        wire::AgentActivityKind::Reasoning => "reasoning",
        wire::AgentActivityKind::Turn => "turn",
        wire::AgentActivityKind::Compaction => "compaction",
    }
}

/// Exact displayed width of every relative-time slot, in CSS `ch` units.
pub const AGE_SLOT_WIDTH: usize = 3;

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
    relative_label(millis)
}

/// A remaining duration in the band's compact spelling.
///
/// Expired and sub-second deadlines read as `now`: no positive whole unit
/// remains. Positive durations use the same unit boundaries and fixed-width
/// budget as elapsed ages so replacing renewal age with expiry countdown does
/// not move neighboring controls.
#[must_use]
pub fn remaining_label(millis: i64) -> String {
    relative_label(millis)
}

fn relative_label(millis: i64) -> String {
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

/// Relative task clocks remain precise through hours, then yield to the
/// browser's localized absolute timestamp instead of rounding into days.
#[must_use]
pub fn task_elapsed_label(millis: i64) -> Option<String> {
    (millis < MILLIS_PER_DAY).then(|| elapsed_label(millis))
}

/// Relative task deadlines remain precise through hours, then yield to the
/// browser's localized absolute timestamp instead of rounding into days.
#[must_use]
pub fn task_remaining_label(millis: i64) -> Option<String> {
    (millis < MILLIS_PER_DAY).then(|| remaining_label(millis))
}

/// Delay until a task age's visible relative spelling changes or becomes
/// absolute at the 24-hour boundary.
#[must_use]
pub fn task_elapsed_label_deadline_millis(millis: i64) -> Option<u32> {
    (millis < MILLIS_PER_DAY)
        .then(|| elapsed_label_deadline_millis(millis))
        .flatten()
}

/// Delay until a task deadline's absolute spelling becomes relative, or its
/// visible relative spelling changes.
#[must_use]
pub fn task_remaining_label_deadline_millis(millis: i64) -> Option<u32> {
    if millis >= MILLIS_PER_DAY {
        return Some(
            u32::try_from(
                i128::from(millis)
                    .saturating_sub(i128::from(MILLIS_PER_DAY))
                    .saturating_add(1)
                    .clamp(MIN_TIMER_DELAY_MILLIS, i128::from(u32::MAX)),
            )
            .expect("task deadline delay is clamped to u32"),
        );
    }
    remaining_label_deadline_millis(millis)
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

/// Delay until a decreasing [`remaining_label`] can return a new spelling.
///
/// Unlike an elapsed age, an expired deadline never changes without a new
/// authoritative expiry. Far-future saturated labels still arm a bounded timer
/// because they eventually descend from `99d+` into the ordinary day range.
#[must_use]
pub fn remaining_label_deadline_millis(millis: i64) -> Option<u32> {
    let seconds = millis / MILLIS_PER_SECOND;
    let delay = match seconds {
        ..=0 => return None,
        1..=LAST_SECOND_OF_MINUTE => i128::from(until_previous_multiple(millis, MILLIS_PER_SECOND)),
        SECONDS_PER_MINUTE..=LAST_SECOND_OF_HOUR => {
            i128::from(until_previous_multiple(millis, MILLIS_PER_MINUTE))
        }
        SECONDS_PER_HOUR..=LAST_SECOND_OF_DAY => {
            i128::from(until_previous_multiple(millis, MILLIS_PER_HOUR))
        }
        _ if seconds / SECONDS_PER_DAY <= AGE_LABEL_MAX_DAYS => {
            i128::from(until_previous_multiple(millis, MILLIS_PER_DAY))
        }
        _ => i128::from(
            millis
                .saturating_sub((AGE_LABEL_MAX_DAYS + 1) * MILLIS_PER_DAY)
                .saturating_add(1),
        ),
    };
    Some(
        u32::try_from(delay.clamp(MIN_TIMER_DELAY_MILLIS, i128::from(u32::MAX)))
            .expect("delay is clamped to u32"),
    )
}

fn until_previous_multiple(millis: i64, quantum: i64) -> i64 {
    millis.rem_euclid(quantum).saturating_add(1)
}

fn until_next_multiple(millis: i64, quantum: i64) -> i64 {
    quantum - millis.rem_euclid(quantum)
}

/// First authored line after borrowed Markdown quote context is skipped.
///
/// Composer headers are intentionally prose-led rather than identity-led. A
/// reply can begin with blockquotes, canonical structural quote links, or the
/// explicit marker produced when one of those links is unavailable, so the
/// first raw line is not necessarily something the resident wrote. Trimming
/// before classification handles indented context, and returning `None` for a
/// message containing only context lets the caller continue into older messages.
#[must_use]
pub fn first_nonquoted_prose(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with('>')
                && wire::QuoteTarget::from_markdown(line).is_err()
                && !is_unavailable_quote_context(line)
        })
        .map(ToOwned::to_owned)
}

/// Whether `line` is the exact structural marker emitted for a failed quote.
fn is_unavailable_quote_context(line: &str) -> bool {
    let Some(body) = line.strip_prefix("[quote unavailable: reason=") else {
        return false;
    };
    let Some((reason, source)) = body.split_once(" source=") else {
        return false;
    };
    matches!(
        reason,
        "malformed" | "unknown" | "unreadable" | "digest-mismatch" | "depth-exceeded"
    ) && source.starts_with("\"spice://quote/")
        && source.ends_with("\"]")
}

/// Newest prose one lane authored, paired with the line its header shows.
///
/// A composer header speaks for exactly one agent, so both its text and the
/// age beside it come from that lane and from nowhere else. A lane that has
/// produced no prose within `messages` yields `None`, and the caller prints
/// the empty header rather than borrowing a peer's message: a vacant lane
/// that has never run must not wear the most recent active agent's status.
///
/// Prose is lane-scoped rather than occupant-scoped. A message a prior
/// occupant authored in the same lane remains that lane's own prose, because
/// the lane is the surface the operator addresses. Tool previews, reasoning,
/// turn starts, compaction boundaries, and entirely quoted replies are
/// skipped in favour of older prose, as the header labels the sentence it
/// prints. `messages` is read in creation order; the newest eligible one wins.
#[must_use]
pub fn latest_lane_prose<'a>(
    lane: CanonicalUuid,
    messages: impl DoubleEndedIterator<Item = &'a wire::AgentMessage>,
) -> Option<(&'a wire::AgentMessage, String)> {
    messages
        .rev()
        .filter(|message| {
            message.lane == lane && message.activity == wire::AgentActivityKind::Prose
        })
        .find_map(|message| first_nonquoted_prose(&message.text).map(|prose| (message, prose)))
}

/// Display the producer's preview or the explicit absent-preview label.
///
/// This is the boundary between a producer and every surface that voices an
/// agent's prose as its own: the composer quote header and the team ticker
/// both pass the outcome of [`first_nonquoted_prose`] here rather than
/// reparsing authored text, so an entirely quoted message wears one shared
/// label instead of a borrowed quote presented as the agent's own words.
#[must_use]
pub fn quote_label(preview: Option<&str>) -> &str {
    preview.unwrap_or("quoted message")
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
/// the fixed-width base-52 task stamp. Repeated mentions produce one reported
/// identity even though the browser renders every authored occurrence.
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

/// Concise identity and detail chunks for one status ticker row.
#[must_use]
pub fn ticker_chunks(class: &str, detail: &str, budget: usize) -> (String, String) {
    let detail = one_line(detail);
    if class == "tool" {
        let line = truncate(&detail, budget);
        let (prefix, detail) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        return (
            prefix.strip_suffix(':').unwrap_or(prefix).to_owned(),
            detail.to_owned(),
        );
    }
    let (prefix, detail) = (class, detail.as_str());
    let line = ticker_line(prefix, detail, budget);
    let marker = format!("{prefix}:");
    let visible_detail = line
        .strip_prefix(&marker)
        .map(str::trim_start)
        .unwrap_or_default();
    (prefix.to_owned(), visible_detail.to_owned())
}

/// Preserve standing and activity identity inside one bounded assistive label.
#[must_use]
pub fn ticker_accessible_label(label: &str, class: &str, line: &str, budget: usize) -> String {
    let line = if class == "tool" {
        format!("tool: {line}")
    } else {
        line.to_owned()
    };
    ticker_line(label, &line, budget)
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
        MAX_TEAM_VISUAL_WEIGHT, MILLIS_PER_DAY, agent_message_attribution, agent_message_label,
        lane_accent, quote_label, swap_members, task_elapsed_label,
        task_elapsed_label_deadline_millis, task_remaining_label,
        task_remaining_label_deadline_millis, team_visual_weight, ticker_accessible_label,
        ticker_chunks, tool_media_description, transfer_member,
    };

    fn id(suffix: u8) -> wire::CanonicalUuid {
        format!("00000000-0000-4000-8000-{suffix:012}")
            .parse()
            .expect("fixture identity is canonical")
    }

    fn member(suffix: u8) -> crate::runtime::LaneMember {
        crate::runtime::LaneMember {
            lane: id(suffix),
            name: None,
            worktree: format!("/work/lane-{suffix}"),
            speech: wire::SpeechPlayback::Disabled {},
            life: "steer".to_owned(),
            vendor: "omp".parse().expect("fixture vendor is canonical"),
            model: None,
            effort: None,
            occupant: None,
            session: None,
            disposition: "seated".to_owned(),
            liveness: wire::intent::LaneLiveness::Vacant,
            bound: false,
            wakeable: true,
            jobs: Vec::new(),
        }
    }

    #[test]
    fn task_times_cross_from_relative_hours_to_absolute_at_one_day() {
        assert_eq!(
            task_elapsed_label(MILLIS_PER_DAY - 1),
            Some("23h".to_owned())
        );
        assert_eq!(task_elapsed_label(MILLIS_PER_DAY), None);
        assert_eq!(
            task_remaining_label(MILLIS_PER_DAY - 1),
            Some("23h".to_owned())
        );
        assert_eq!(task_remaining_label(MILLIS_PER_DAY), None);

        assert_eq!(
            task_elapsed_label_deadline_millis(MILLIS_PER_DAY - 1),
            Some(1)
        );
        assert_eq!(task_elapsed_label_deadline_millis(MILLIS_PER_DAY), None);
        assert_eq!(
            task_remaining_label_deadline_millis(MILLIS_PER_DAY),
            Some(1)
        );
        assert_eq!(
            task_remaining_label_deadline_millis(MILLIS_PER_DAY + 999),
            Some(1_000)
        );
        assert_eq!(task_remaining_label(-1), Some("now".to_owned()));
        assert_eq!(task_remaining_label_deadline_millis(-1), None);
    }

    #[test]
    fn lane_accent_resolves_the_same_right_edge_palette_used_by_agent_surfaces() {
        let lanes = [crate::runtime::LaneRow {
            lane: id(10),
            subscription: wire::intent::TeamLife::Steer,
            members: vec![member(1), member(2), member(3)],
        }];

        assert_eq!(lane_accent(&lanes, id(1)), Some(2));
        assert_eq!(lane_accent(&lanes, id(2)), Some(1));
        assert_eq!(lane_accent(&lanes, id(3)), Some(0));
        assert_eq!(lane_accent(&lanes, id(4)), None);
    }

    #[test]
    fn composer_swap_changes_only_the_two_hovered_slots() {
        let topology = vec![
            (id(10), vec![id(1), id(2), id(3)]),
            (id(20), vec![id(4), id(5)]),
        ];

        assert_eq!(
            swap_members(&topology, id(1), id(3)),
            Some(vec![
                (id(10), vec![id(3), id(2), id(1)]),
                (id(20), vec![id(4), id(5)]),
            ]),
        );
        assert_eq!(swap_members(&topology, id(2), id(5)), None);
        assert_eq!(swap_members(&topology, id(2), id(2)), None);
        assert_eq!(swap_members(&topology, id(2), id(9)), None);
    }

    #[test]
    fn composer_transfer_appends_to_the_destination_and_drops_an_emptied_team() {
        let topology = vec![
            (id(10), vec![id(1), id(2), id(3)]),
            (id(20), vec![id(4), id(5)]),
            (id(30), vec![id(6)]),
        ];

        assert_eq!(
            transfer_member(&topology, id(1), id(20)),
            Some(vec![
                (id(10), vec![id(2), id(3)]),
                (id(20), vec![id(4), id(5), id(1)]),
                (id(30), vec![id(6)]),
            ]),
            "the destination keeps its first agent and the source its order",
        );
        assert_eq!(
            transfer_member(&topology, id(6), id(10)),
            Some(vec![
                (id(10), vec![id(1), id(2), id(3), id(6)]),
                (id(20), vec![id(4), id(5)]),
            ]),
        );
        assert_eq!(transfer_member(&topology, id(2), id(10)), None);
        assert_eq!(transfer_member(&topology, id(2), id(99)), None);
    }

    #[test]
    fn absent_preview_has_one_documented_label() {
        assert_eq!(quote_label(Some("author words")), "author words");
        assert_eq!(quote_label(None), "quoted message");
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
            "Ash",
        );
    }

    #[test]
    fn unnamed_message_label_uses_the_worktree_basename() {
        assert_eq!(
            agent_message_label("/fleet/one/spice-rust-z/", None),
            "spice-rust-z",
        );
    }

    #[test]
    fn message_label_names_missing_identity_once() {
        assert_eq!(
            agent_message_label("/", Some("  ")),
            super::UNKNOWN_AGENT_LABEL
        );
        assert_eq!(agent_message_label("", None), super::UNKNOWN_AGENT_LABEL);
    }

    #[test]
    fn team_visual_weight_has_a_nonzero_six_share_ceiling() {
        assert_eq!(team_visual_weight(0), 1);
        assert_eq!(team_visual_weight(1), 1);
        assert_eq!(team_visual_weight(MAX_TEAM_VISUAL_WEIGHT), 6);
        assert_eq!(team_visual_weight(7), 6);
        assert_eq!(team_visual_weight(usize::MAX), 6);
    }

    #[test]
    fn tool_ticker_accent_ends_only_at_the_first_whitespace_boundary() {
        for (detail, prefix, remainder) in [
            (
                "cargo test:all --url https://host:443",
                "cargo",
                "test:all --url https://host:443",
            ),
            ("shell: command", "shell", "command"),
            ("https://host:443/run now", "https://host:443/run", "now"),
            ("src/main.rs:42:7 changed", "src/main.rs:42:7", "changed"),
            (
                r#"echo "left:right" '{"key":"a:b:c"}'"#,
                "echo",
                r#""left:right" '{"key":"a:b:c"}'"#,
            ),
            ("single:token", "single:token", ""),
        ] {
            assert_eq!(
                ticker_chunks("tool", detail, 120),
                (prefix.to_owned(), remainder.to_owned()),
                "{detail}",
            );
        }
    }

    #[test]
    fn non_tool_ticker_chunking_keeps_its_existing_colon_contract() {
        assert_eq!(
            ticker_chunks("assistant", "working through: details", 120),
            (
                "assistant".to_owned(),
                "working through: details".to_owned()
            ),
        );
    }

    #[test]
    fn tool_ticker_accessibility_retains_the_suppressed_activity_class() {
        assert_eq!(
            ticker_accessible_label("Agent", "tool", "exec completed", 120),
            "Agent: tool: exec completed"
        );
        assert_eq!(
            ticker_accessible_label("Agent", "assistant", "assistant: working", 120),
            "Agent: assistant: working"
        );
        let bounded = ticker_accessible_label("Agent", "tool", &"é".repeat(120), 120);
        assert_eq!(bounded.chars().count(), 120);
        assert!(bounded.starts_with("Agent: tool: "));
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn an_image_description_keeps_the_preview_line_with_paths_cut_to_base_names() {
        assert_eq!(
            tool_media_description(
                "read: /tmp/spice-pi-badge-visual-k/canvas-vendor-emblem-light.png succeeded"
            ),
            "read: canvas-vendor-emblem-light.png succeeded"
        );
        assert_eq!(
            tool_media_description("file change: spice/src/a.rs, app/mosaic.css (add) succeeded"),
            "file change: a.rs, mosaic.css (add) succeeded"
        );
        assert_eq!(
            tool_media_description("cat --output=/tmp/shots/x.png | tee ~/keep/y.png completed"),
            "cat --output=x.png | tee y.png completed"
        );
    }

    #[test]
    fn an_image_description_keeps_the_call_and_quotes_around_a_quoted_path() {
        assert_eq!(
            tool_media_description(
                r#"const r = await tools.view_image("/tmp/shots/emblem.png") completed"#
            ),
            r#"const r = await tools.view_image("emblem.png") completed"#
        );
        assert_eq!(
            tool_media_description("view image: capture.png succeeded"),
            "view image: capture.png succeeded"
        );
        assert_eq!(tool_media_description("ls / succeeded"), "ls / succeeded");
        assert_eq!(
            tool_media_description("ls shots/ succeeded"),
            "ls shots succeeded"
        );
        assert_eq!(tool_media_description(""), "");
    }
}
