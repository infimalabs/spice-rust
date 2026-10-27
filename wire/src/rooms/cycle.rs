//! Proposal-backed Design pass cycle.
//!
//! An open Design alternates between passes and the input those passes need.
//! A pass is a claim won in the design phase; releasing an intake claim ends
//! none. A pass ends at its boundary: the claim release that a proposal
//! recorded, or a proposal-less free or lapse that abandoned it. Before any
//! boundary the Design is ready only when its maturity is `Steeped` or a
//! retained operator nudge readied it. After a boundary only counted input
//! recorded later refills it: a typed contribution, an operator-authored room
//! post, a retained nudge, decision feedback that returned or rejected work,
//! or the final settlement
//! of released work. While work the Design released is still running, the
//! Design waits on it whatever input arrives: an approved proposal is worked
//! whole before another pass opens, and the released work is the Design's
//! prerequisite set. Requests, notices, asks, leaves, group membership, and
//! unrelated facts are inert.
//!
//! [`DesignCycles`] folds those authorities in journal order, one fact at a
//! time, so replay is bounded by the facts it reads and never rescans a room.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{
    CandidateRef, CandidateWithdrawn, DecisionOutcome, DesignItems, DesignProposed,
    ExecutionAuthorized, OutputControlled, OutputVerdict, PreparationDecided, RoomContributed,
    RoomDestination, RoomPosted, RoomPublicationContent, RoomPublished,
};
use crate::body::{
    ClaimFreed, ClaimLapsed, ClaimWon, TaskAdded, TaskClosed, TaskCompletionStatus, TaskConcluded,
    TaskDeleted, TaskDone, TaskReviewed,
};
use crate::{Actor, AnyFact, CanonicalUuid, FactKind, Seq, SequenceCode, TaskHandle, TaskStamp};

/// The operator asked one open Design to take its next pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignNudged {
    /// The nudged Design.
    pub design: TaskStamp,
    /// The operator MSG the same write sent one lane in the Design's room,
    /// carrying the room's unheard operator posts; absent when no lane could
    /// take the Design.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<super::RoomAuthorityStamp>,
    /// The lane the message went to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<CanonicalUuid>,
}

/// What one open Design's pass cycle waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignCycle {
    /// No counted input has arrived since the last pass boundary.
    GatheringInput,
    /// Counted input arrived; a lane may claim the Design and propose.
    ReadyForProposal,
    /// A proposal the operator has not decided or its author withdrawn.
    AwaitingDecision,
    /// Accepted work is still running; the Design waits on it as its
    /// prerequisites, whatever input arrives meanwhile.
    WaitingOnSelectedWork,
}

/// Who acts next on one Design's pass cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignCycleActor {
    /// Contributing lanes or the operator supply input.
    Contributors,
    /// A lane claims the Design and records one proposal.
    Lane,
    /// The operator decides the exact proposal.
    Operator,
    /// The work the Design released finishes.
    SelectedWork,
}

impl DesignCycleActor {
    /// Operator-facing name of who acts.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Contributors => "contributors",
            Self::Lane => "lane",
            Self::Operator => "operator",
            Self::SelectedWork => "selected work",
        }
    }
}

impl DesignCycle {
    /// Every state, in cycle order.
    pub const ALL: [Self; 4] = [
        Self::GatheringInput,
        Self::ReadyForProposal,
        Self::AwaitingDecision,
        Self::WaitingOnSelectedWork,
    ];

    /// Operator-facing name of this state.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GatheringInput => "gathering input",
            Self::ReadyForProposal => "ready for proposal",
            Self::AwaitingDecision => "awaiting decision",
            Self::WaitingOnSelectedWork => "waiting on selected work",
        }
    }

    /// Stable kebab-case key, the state's serialized name.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::GatheringInput => "gathering-input",
            Self::ReadyForProposal => "ready-for-proposal",
            Self::AwaitingDecision => "awaiting-decision",
            Self::WaitingOnSelectedWork => "waiting-on-selected-work",
        }
    }

    /// Who moves this Design forward.
    #[must_use]
    pub const fn next_actor(self) -> DesignCycleActor {
        match self {
            Self::GatheringInput => DesignCycleActor::Contributors,
            Self::ReadyForProposal => DesignCycleActor::Lane,
            Self::AwaitingDecision => DesignCycleActor::Operator,
            Self::WaitingOnSelectedWork => DesignCycleActor::SelectedWork,
        }
    }

    /// The exact action that moves `design` forward, citing the proposal or
    /// decision position `facts` holds.
    #[must_use]
    pub fn next_action(self, design: &TaskHandle, facts: DesignCycleFacts) -> String {
        match self {
            Self::GatheringInput => format!(
                "contribute with `spice room contribute {design}`, post in its room, or have \
                 a lane claim it now with `spice task capture {design}`"
            ),
            Self::ReadyForProposal => format!(
                "a lane claims {design} through `spice task next` and ends its pass with one \
                 `spice design propose JUSTIFICATION`"
            ),
            Self::AwaitingDecision => facts.proposal.map_or_else(
                || format!("the operator decides the proposal under {design}"),
                |proposal| {
                    format!(
                        "the operator decides proposal {} under {design} with `spice design \
                         decide DECISION_JSON`",
                        SequenceCode::new(proposal.get())
                    )
                },
            ),
            Self::WaitingOnSelectedWork => facts.decision.map_or_else(
                || format!("finish the work {design} released"),
                |decision| {
                    format!(
                        "finish the work decision {} of {design} selected",
                        SequenceCode::new(decision.get())
                    )
                },
            ),
        }
    }
}

/// One open Design's pass cycle as every surface presents it: the state,
/// who acts, the exact action, how the latest pass ended, and the proposal
/// and decision positions that action cites.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignCyclePresentation {
    /// What the cycle waits on.
    pub cycle: DesignCycle,
    /// Who acts next.
    pub actor: DesignCycleActor,
    /// The exact action that moves the Design forward.
    pub action: String,
    /// How the latest pass ended its claim, once any pass has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pass: Option<DesignPassEnd>,
    /// The oldest proposal awaiting the operator's decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<SequenceCode>,
    /// The operator's latest decision of one of its proposals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<SequenceCode>,
}

/// Whether a lane may take, or already holds, one open Design's pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesignAvailability {
    /// Deferred without a holder; waking may atomically claim its pass.
    DeferredUnclaimed,
    /// Deferred while its holder checkpoints and frees its pass.
    DeferredClaimed,
    /// Undeferred and unclaimed.
    Unclaimed,
    /// A lane holds its current pass.
    Claimed,
}

impl DesignCyclePresentation {
    /// Present `design` in `cycle` with the evidence `facts` carries. A
    /// deferred, unheld Design may be woken and claimed atomically; a deferred
    /// held Design remains its holder's responsibility until that lane frees
    /// the pass.
    #[must_use]
    pub fn new(
        cycle: DesignCycle,
        facts: DesignCycleFacts,
        design: &TaskHandle,
        availability: DesignAvailability,
    ) -> Self {
        let code = |seq: Seq| SequenceCode::new(seq.get());
        let (actor, action) = match (cycle, availability) {
            (
                DesignCycle::GatheringInput | DesignCycle::ReadyForProposal,
                DesignAvailability::DeferredUnclaimed,
            ) => (
                DesignCycleActor::Lane,
                format!(
                    "a lane wakes and atomically claims {design} with `spice task wake {design} \
                     --claim`"
                ),
            ),
            (
                DesignCycle::GatheringInput | DesignCycle::ReadyForProposal,
                DesignAvailability::DeferredClaimed,
            ) => (
                DesignCycleActor::Lane,
                format!(
                    "the holding lane checkpoints its current work and runs `spice task free \
                     {design}`; its claim stays authoritative until then"
                ),
            ),
            (
                DesignCycle::ReadyForProposal | DesignCycle::GatheringInput,
                DesignAvailability::Claimed,
            ) => (
                DesignCycleActor::Lane,
                format!(
                    "the holding lane ends this pass of {design} with one `spice design \
                     propose JUSTIFICATION`, which releases its claim"
                ),
            ),
            _ => (cycle.next_actor(), cycle.next_action(design, facts)),
        };
        Self {
            cycle,
            actor,
            action,
            last_pass: facts.end,
            proposal: facts.proposal.map(code),
            decision: facts.decision.map(code),
        }
    }

    /// Plain-text lines naming the state, actor, latest pass, and cited
    /// authorities; callers print [`Self::action`] as their next step.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("cycle {}", self.cycle.label()),
            format!("cycle-actor {}", self.actor.label()),
        ];
        lines.extend(
            self.last_pass
                .map(|end| format!("last-pass {}", end.label())),
        );
        lines.extend(self.proposal.map(|code| format!("proposal {code}")));
        lines.extend(self.decision.map(|code| format!("decision {code}")));
        lines
    }
}

/// How many open Designs stand in each pass-cycle state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignCycleCounts {
    /// Designs gathering input.
    pub gathering_input: usize,
    /// Designs ready for a proposal.
    pub ready_for_proposal: usize,
    /// Designs awaiting the operator's decision.
    pub awaiting_decision: usize,
    /// Designs waiting on the work they selected.
    pub waiting_on_selected_work: usize,
}

impl DesignCycleCounts {
    /// Count one Design in `cycle`.
    pub fn observe(&mut self, cycle: DesignCycle) {
        *self.slot(cycle) += 1;
    }

    /// Designs counted in `cycle`.
    #[must_use]
    pub fn get(mut self, cycle: DesignCycle) -> usize {
        *self.slot(cycle)
    }

    fn slot(&mut self, cycle: DesignCycle) -> &mut usize {
        match cycle {
            DesignCycle::GatheringInput => &mut self.gathering_input,
            DesignCycle::ReadyForProposal => &mut self.ready_for_proposal,
            DesignCycle::AwaitingDecision => &mut self.awaiting_decision,
            DesignCycle::WaitingOnSelectedWork => &mut self.waiting_on_selected_work,
        }
    }
}

/// How the latest pass boundary ended its claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignPassEnd {
    /// The pass recorded a proposal and released its claim with it.
    Proposed,
    /// The claim was freed or lapsed with no proposal.
    Abandoned,
}

impl DesignPassEnd {
    /// Operator-facing name of how the pass ended.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Abandoned => "abandoned",
        }
    }
}

/// Compact cycle evidence one Design carries into allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignCycleFacts {
    /// The Design's maturity is `Steeped` at its current statement.
    #[serde(default)]
    pub steeped: bool,
    /// The operator nudged the Design at least once.
    #[serde(default)]
    pub nudged: bool,
    /// The Design is an OOPS report intake: each report filed into it is
    /// counted input, so its first pass needs no steeping.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub intake: bool,
    /// Latest pass boundary: a proposal or a claim release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<Seq>,
    /// How the latest claim release ended its pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DesignPassEnd>,
    /// Latest counted input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Seq>,
    /// Latest settlement of work this Design released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<Seq>,
    /// Position of the oldest proposal still awaiting the operator's
    /// decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Seq>,
    /// Position of the operator's latest decision of one of its proposals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Seq>,
}

impl DesignCycleFacts {
    /// Whether no cycle authority has touched this task.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The cycle state, given whether work this Design released is still
    /// running, released or deferred. Released work holds the Design until
    /// it settles: the approved proposal is worked whole before input can
    /// open another pass.
    #[must_use]
    pub fn cycle(self, working: bool) -> DesignCycle {
        if self.proposal.is_some() {
            DesignCycle::AwaitingDecision
        } else if working {
            DesignCycle::WaitingOnSelectedWork
        } else if self.counted() {
            DesignCycle::ReadyForProposal
        } else {
            DesignCycle::GatheringInput
        }
    }

    fn counted(self) -> bool {
        let Some(horizon) = self.horizon else {
            return self.steeped || self.nudged || self.intake && self.input.is_some();
        };
        self.input.is_some_and(|input| input > horizon)
            || self.settled.is_some_and(|settled| settled > horizon)
    }
}

/// One Design's fold state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DesignCycleFold {
    /// Each charged question item's series; steeping is judged against the
    /// current statement when the facts are read.
    items: DesignItems,
    /// Proposals still awaiting the operator's decision, oldest first, each
    /// with the position that recorded it.
    open: Vec<(CandidateRef, Seq)>,
    /// A proposal arrived since the latest claim release.
    proposed: bool,
    facts: DesignCycleFacts,
}

impl DesignCycleFold {
    fn input(&mut self, seq: Seq) {
        self.facts.input = Some(seq);
    }

    fn boundary(&mut self, seq: Seq) {
        self.facts.horizon = Some(self.facts.horizon.map_or(seq, |horizon| horizon.max(seq)));
    }

    fn contributed(&mut self, seq: Seq, contribution: &RoomContributed) {
        self.input(seq);
        self.items.observe(contribution);
    }

    fn proposed(&mut self, seq: Seq, candidate: &CandidateRef) {
        self.boundary(seq);
        if !self.open.iter().any(|(open, _)| open == candidate) {
            self.open.push((candidate.clone(), seq));
        }
        self.proposed = true;
        self.facts.proposal = self.open.first().map(|(_, seq)| *seq);
    }

    fn decided(&mut self, seq: Seq, outcome: &DecisionOutcome) {
        self.settle(outcome.candidate());
        self.facts.decision = Some(seq);
        let feedback = match outcome {
            DecisionOutcome::Returned { .. } => true,
            DecisionOutcome::Resolved { outputs, .. } => outputs
                .iter()
                .any(|output| output.verdict == OutputVerdict::Rejected),
            DecisionOutcome::Accepted { .. } => false,
        };
        if feedback {
            self.input(seq);
        }
    }

    /// A decision or withdrawal settles `candidate`; any other proposal still
    /// open keeps the operator's decision owed.
    fn settle(&mut self, candidate: &CandidateRef) {
        self.open.retain(|(open, _)| open != candidate);
        self.facts.proposal = self.open.first().map(|(_, seq)| *seq);
    }

    fn released(&mut self, seq: Seq) {
        self.boundary(seq);
        self.facts.end = Some(if self.proposed {
            DesignPassEnd::Proposed
        } else {
            DesignPassEnd::Abandoned
        });
        self.proposed = false;
    }
}

/// Pass-cycle evidence for every Design, folded from journal facts in order.
///
/// A pass is a claim held in the design phase: only a release of a claim won
/// there is a pass boundary, so releasing an intake claim, before or after the
/// phase change into design, leaves the first pass gated on maturity or a
/// nudge. A settled task's fold is dropped. Released work maps each authorized
/// output or Plan-constructed task to its controlling Design so its settlement
/// reaches that Design.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesignCycles {
    designs: HashMap<TaskStamp, DesignCycleFold>,
    released_work: HashMap<TaskStamp, TaskStamp>,
    /// Tasks whose live claim was won in the design phase.
    passes: HashSet<TaskStamp>,
}

impl DesignCycles {
    /// Whether `kind` can change any Design's cycle evidence.
    #[must_use]
    pub const fn concerns(kind: FactKind) -> bool {
        matches!(
            kind,
            FactKind::RoomContributed
                | FactKind::RoomPublished
                | FactKind::RoomPosted
                | FactKind::DesignNudged
                | FactKind::DesignProposed
                | FactKind::PreparationDecided
                | FactKind::CandidateWithdrawn
                | FactKind::ExecutionAuthorized
                | FactKind::TaskAdded
                | FactKind::OutputControlled
                | FactKind::ClaimWon
                | FactKind::ClaimFreed
                | FactKind::ClaimLapsed
                | FactKind::TaskDone
                | FactKind::TaskReviewed
                | FactKind::TaskClosed
                | FactKind::TaskDeleted
                | FactKind::TaskConcluded
        )
    }

    /// Current evidence for `design` whose current statement items are
    /// `acceptance`; empty when nothing has touched it.
    #[must_use]
    pub fn facts(&self, design: &TaskStamp, acceptance: &[String]) -> DesignCycleFacts {
        self.designs
            .get(design)
            .map(|fold| DesignCycleFacts {
                steeped: fold.items.steeped(acceptance),
                ..fold.facts
            })
            .unwrap_or_default()
    }

    /// Each charged question item's series for `design`, if anything touched
    /// it.
    #[must_use]
    pub fn items(&self, design: &TaskStamp) -> Option<&DesignItems> {
        self.designs.get(design).map(|fold| &fold.items)
    }

    /// Every Design whose cycle evidence `fact` may change and that the fact
    /// does not already name as its task subject or a mutated task.
    ///
    /// Settled released work reaches its controller through the settled
    /// task's own preparation, so a caller refreshing cached Designs also
    /// refreshes each changed task's controller.
    #[must_use]
    pub fn subjects(fact: &AnyFact) -> Vec<&TaskStamp> {
        if let Some(contribution) = fact.as_body::<RoomContributed>() {
            vec![&contribution.room]
        } else if let Some(published) = fact.as_body::<RoomPublished>() {
            vec![&published.room]
        } else if let Some(posted) = fact.as_body::<RoomPosted>() {
            posted_rooms(posted)
        } else if let Some(nudged) = fact.as_body::<DesignNudged>() {
            vec![&nudged.design]
        } else if let Some(proposed) = fact.as_body::<DesignProposed>() {
            vec![&proposed.candidate.candidate.controller]
        } else if let Some(decided) = fact.as_body::<PreparationDecided>() {
            vec![&decided.decision.outcome.candidate().controller]
        } else if let Some(withdrawn) = fact.as_body::<CandidateWithdrawn>() {
            vec![&withdrawn.withdrawal.candidate.controller]
        } else if let Some(added) = fact.as_body::<TaskAdded>() {
            added
                .preparation
                .iter()
                .map(|preparation| &preparation.controller)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Fold one fact at journal position `seq` authored by `actor`.
    pub fn observe(&mut self, seq: Seq, actor: &Actor, fact: &AnyFact) {
        match fact.kind() {
            FactKind::RoomContributed
            | FactKind::RoomPublished
            | FactKind::RoomPosted
            | FactKind::DesignNudged => self.observe_input(seq, actor, fact),
            FactKind::DesignProposed
            | FactKind::PreparationDecided
            | FactKind::CandidateWithdrawn
            | FactKind::ExecutionAuthorized => self.observe_proposal(seq, fact),
            FactKind::TaskAdded => self.observe_added(seq, fact),
            FactKind::OutputControlled => self.observe_control(fact),
            FactKind::ClaimWon
            | FactKind::ClaimFreed
            | FactKind::ClaimLapsed
            | FactKind::TaskDone
            | FactKind::TaskReviewed
            | FactKind::TaskClosed
            | FactKind::TaskDeleted
            | FactKind::TaskConcluded => self.observe_task(seq, fact),
            _ => {}
        }
    }

    fn design(&mut self, design: &TaskStamp) -> &mut DesignCycleFold {
        self.designs.entry(design.clone()).or_default()
    }

    fn observe_input(&mut self, seq: Seq, actor: &Actor, fact: &AnyFact) {
        if let Some(contribution) = fact.as_body::<RoomContributed>() {
            self.design(&contribution.room)
                .contributed(seq, contribution);
        } else if let Some(nudged) = fact.as_body::<DesignNudged>() {
            let design = self.design(&nudged.design);
            design.facts.nudged = true;
            design.input(seq);
        } else if matches!(actor, Actor::Operator | Actor::Human(_)) {
            self.observe_operator_post(seq, fact);
        }
    }

    /// An operator's authored room post or MSG counts as input.
    fn observe_operator_post(&mut self, seq: Seq, fact: &AnyFact) {
        if let Some(published) = fact.as_body::<RoomPublished>() {
            if matches!(published.content, RoomPublicationContent::Authored { .. }) {
                self.design(&published.room).input(seq);
            }
        } else if let Some(posted) = fact.as_body::<RoomPosted>() {
            for room in posted_rooms(posted) {
                self.design(room).input(seq);
            }
        }
    }

    fn observe_proposal(&mut self, seq: Seq, fact: &AnyFact) {
        if let Some(proposed) = fact.as_body::<DesignProposed>() {
            let candidate = &proposed.candidate.candidate;
            self.design(&candidate.controller).proposed(seq, candidate);
        } else if let Some(decided) = fact.as_body::<PreparationDecided>() {
            let outcome = &decided.decision.outcome;
            self.design(&outcome.candidate().controller)
                .decided(seq, outcome);
        } else if let Some(withdrawn) = fact.as_body::<CandidateWithdrawn>() {
            let candidate = &withdrawn.withdrawal.candidate;
            self.design(&candidate.controller).settle(candidate);
        } else if let Some(authorized) = fact.as_body::<ExecutionAuthorized>() {
            let controller = &authorized.authorization.decision.controller;
            for output in &authorized.authorization.selected_outputs {
                self.released_work
                    .insert(output.clone(), controller.clone());
            }
        }
    }

    /// An OOPS Design opens an intake; each report it is born controlling is
    /// counted input.
    fn observe_added(&mut self, seq: Seq, fact: &AnyFact) {
        if let Some(added) = fact.as_body::<TaskAdded>() {
            if added.intent == Some(crate::TaskIntent::OpportunisticDesign) {
                self.design(&added.task).facts.intake = true;
            }
            if let Some(preparation) = &added.preparation
                && let Some(design) = self.designs.get_mut(&preparation.controller)
                && design.facts.intake
            {
                design.input(seq);
            }
        }
        self.observe_control(fact);
    }

    /// Record Plan-constructed work: a task whose immediate parent is not its
    /// controller was released by that Plan, never by a Design selection.
    fn observe_control(&mut self, fact: &AnyFact) {
        let preparation = if let Some(added) = fact.as_body::<TaskAdded>() {
            added
                .preparation
                .as_ref()
                .map(|preparation| (&added.task, preparation))
        } else {
            fact.as_body::<OutputControlled>()
                .map(|controlled| (&controlled.output, &controlled.preparation))
        };
        if let Some((task, preparation)) = preparation
            && preparation.parent != preparation.controller
        {
            self.released_work
                .insert(task.clone(), preparation.controller.clone());
        }
    }

    /// A claim won in the design phase opens a pass its release ends; a
    /// completed `task::done` or `task::reviewed` positively settles its
    /// task; a phase advance or a finding leaves released work pending.
    fn observe_task(&mut self, seq: Seq, fact: &AnyFact) {
        let released = if let Some(won) = fact.as_body::<ClaimWon>() {
            if won.cell.phase.as_str() == "design" {
                self.passes.insert(won.task.clone());
            } else {
                self.passes.remove(&won.task);
            }
            return;
        } else if let Some(freed) = fact.as_body::<ClaimFreed>() {
            Some(&freed.task)
        } else {
            fact.as_body::<ClaimLapsed>().map(|lapsed| &lapsed.task)
        };
        if let Some(task) = released {
            if self.passes.remove(task) {
                self.design(task).released(seq);
            }
            return;
        }
        let settled = if let Some(done) = fact.as_body::<TaskDone>() {
            if !done
                .terminal
                .as_ref()
                .is_some_and(|terminal| terminal.status == TaskCompletionStatus::Completed)
            {
                return;
            }
            &done.task
        } else if let Some(reviewed) = fact.as_body::<TaskReviewed>() {
            if reviewed.status != TaskCompletionStatus::Completed {
                return;
            }
            &reviewed.task
        } else if let Some(closed) = fact.as_body::<TaskClosed>() {
            &closed.task
        } else if let Some(deleted) = fact.as_body::<TaskDeleted>() {
            &deleted.task
        } else if let Some(concluded) = fact.as_body::<TaskConcluded>() {
            &concluded.task
        } else {
            return;
        };
        self.designs.remove(settled);
        self.passes.remove(settled);
        if let Some(controller) = self.released_work.remove(settled)
            && let Some(design) = self.designs.get_mut(&controller)
        {
            design.facts.settled = Some(seq);
        }
    }
}

/// Each distinct room an operator post addresses: task routes and route
/// contexts.
fn posted_rooms(posted: &RoomPosted) -> Vec<&TaskStamp> {
    let mut rooms = posted
        .routes
        .iter()
        .flat_map(|route| {
            let destination = match &route.destination {
                RoomDestination::Task { task } => Some(task),
                RoomDestination::Operator {} | RoomDestination::Lane { .. } => None,
            };
            destination.into_iter().chain(route.context.as_ref())
        })
        .collect::<Vec<_>>();
    rooms.sort_unstable();
    rooms.dedup();
    rooms
}

#[cfg(test)]
mod tests;
