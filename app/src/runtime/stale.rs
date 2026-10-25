//! Which current reads a batch of admitted facts invalidated.

use std::collections::BTreeSet;

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
use super::AppRuntime;
use super::TaskRelationInvalidation;
use crate::LaneView;
use crate::socket::Admitted;
use wire::intent::LaneLiveness;
use wire::{CanonicalUuid, FactKind, StreamFrame};

/// Which cached reads a batch of admitted facts has invalidated.
///
/// The stream carries facts, not snapshots: it advances the journal cursor and
/// leaves every read the surface is drawn from exactly as stale as it was.
/// That is not survivable for the watchband, which claims to show what a lane
/// is doing right now. Uncorrected, the shell reports itself online with a
/// current cursor while a tile shows a directive two events old — worse than
/// showing nothing, because the staleness is invisible.
///
/// This lives beside [`AppRuntime::apply_board`], [`AppRuntime::apply_lanes`],
/// [`AppRuntime::apply_inbox`] — one field per read it names — rather than
/// in the socket loop that acts on it, so it can be tested off `wasm32`.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Stale {
    board: bool,
    lanes: bool,
    inbox: bool,
    consent: bool,
    settings: bool,
    principals: bool,
    task_details: BTreeSet<wire::TaskStamp>,
    task_relations: Vec<TaskRelationInvalidation>,
}

impl Stale {
    /// Widen this set by whatever one admitted fact invalidated.
    ///
    /// The fact namespace selects broad snapshot reads here, except for the
    /// facts outside the task namespace that still move task heads. Typed
    /// task relationship semantics live on [`FactKind`] itself; keeping that
    /// finer mapping on the closed vocabulary prevents a second app-local
    /// registry from drifting when task facts are added.
    pub fn widen(&mut self, kind: FactKind) {
        // The board carries every open Design's pass cycle and standing, so a
        // fact that moves cycle evidence (including execution authorization,
        // which also changes allocator eligibility) or the working-group
        // partition and stances moves the board.
        if wire::rooms::DesignCycles::concerns(kind)
            || matches!(
                kind,
                FactKind::WorkingGroupMoved | FactKind::WorkingGroupsArranged
            )
        {
            self.board = true;
        }
        match kind.as_str().split_once("::") {
            Some(("steer", _)) => {
                self.inbox = true;
            }
            // The inbox is keyed on the lane roster, not only on the steer
            // ledger: the resident builds one row per seated lane and carries
            // that lane's occupant on it, so a lane that arrives, departs or
            // changes occupant moves the inbox even though no directive was
            // sent. Widening only `lanes` here left a newly seated lane with
            // no inbox row in the browser, and the console reads its residency
            // witness out of that row -- so the first direction sent to a lane
            // seated from the app was dropped before it reached the wire.
            Some(("lane", _)) => {
                self.lanes = true;
                self.inbox = true;
            }
            Some(("consent", _)) => self.consent = true,
            // The settings read carries every principal's display identity,
            // and the principal listing their roles and capabilities.
            Some(("principal", _)) => {
                self.settings = true;
                self.principals = true;
            }
            Some(("task", _)) => self.board = true,
            // A claim binds a task to a lane, so both sides of that join move.
            Some(("claim", _)) => {
                self.board = true;
                self.lanes = true;
            }
            _ => {}
        }
    }

    fn widen_fact(&mut self, fact: &wire::StreamFact) {
        self.widen(fact.kind());
        if let Some(task) = fact.task() {
            self.task_details.insert(task);
        }
        if let Some(change) = TaskRelationInvalidation::from_fact(fact) {
            if change.kind == FactKind::ExecutionAuthorized {
                self.task_details.insert(change.task.clone());
                self.task_details
                    .extend(change.dependency_targets.iter().cloned());
            }
            self.task_relations.push(change);
        }
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn invalidate_task_details(&self, runtime: &mut AppRuntime, changed: bool) {
        if changed {
            for task in &self.task_details {
                runtime.invalidate_task_detail(task);
            }
        }
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn resolve_task_relations(&mut self, runtime: &mut AppRuntime, changed: bool) {
        if changed {
            runtime.invalidate_task_relations(&self.task_relations);
        }
        self.task_relations.clear();
    }

    /// Affected open task details, in durable identity order.
    pub fn task_details(&self) -> impl Iterator<Item = &wire::TaskStamp> {
        self.task_details.iter()
    }

    /// Re-read a quiet occupant when a live message may have resumed activity.
    ///
    /// The resident still decides liveness; replayed messages cannot make a
    /// process working locally. Rows already heard from need no redundant read.
    pub fn invalidate_quiet_lane(&mut self, view: Option<&LaneView>, lane: CanonicalUuid) {
        self.lanes |= view.is_some_and(|view| {
            view.lanes.iter().any(|team| {
                team.member(lane)
                    .is_some_and(|member| member.liveness == LaneLiveness::Running)
            })
        });
    }

    /// Whether the board read went stale.
    #[must_use]
    pub const fn board(&self) -> bool {
        self.board
    }

    /// Whether the lane read went stale.
    #[must_use]
    pub const fn lanes(&self) -> bool {
        self.lanes
    }

    /// Whether the steering inbox read went stale.
    #[must_use]
    pub const fn inbox(&self) -> bool {
        self.inbox
    }

    /// Whether the operator consent queue read went stale.
    #[must_use]
    pub const fn consent(&self) -> bool {
        self.consent
    }

    /// Whether the settings read, and the principal identities it carries,
    /// went stale.
    #[must_use]
    pub const fn settings(&self) -> bool {
        self.settings
    }

    /// Whether the principal listing went stale.
    #[must_use]
    pub const fn principals(&self) -> bool {
        self.principals
    }

    /// Whether anything needs re-reading at all.
    #[must_use]
    pub fn any(&self) -> bool {
        self.board
            || self.lanes
            || self.inbox
            || self.consent
            || self.settings
            || self.principals
            || !self.task_details.is_empty()
            || !self.task_relations.is_empty()
    }
}
/// Projects current-read invalidation from live facts or complete backlog tails, never history pages.
#[must_use]
pub fn stale_reads(admitted: &Admitted<StreamFrame>) -> Stale {
    let mut stale = Stale::default();
    match admitted.get() {
        StreamFrame::Fact { fact, .. } => stale.widen_fact(fact),
        StreamFrame::Facts { packed, .. } if packed.overflow.is_none() => {
            packed.rows.iter().for_each(|row| stale.widen_fact(row));
        }
        _ => {}
    }
    stale
}
