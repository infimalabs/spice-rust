//! One operator decision over one immutable retained proposal version: record
//! an accept-or-reject verdict and optional comment for every output, or
//! collapse its Design with a reason; preview the effect; then submit once.
//!
//! The draft never changes rendered authority. A successful command answer
//! only reports that the write landed; proposal standing moves when the
//! committed decision and authorization arrive through the room projection.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateRef, DecisionOutcome, DecisionRef, OutputDecision, OutputVerdict, PreparationDecide,
    PreparationDecision, ProposalRef, RetainedCandidate, RoomSource,
};
use wire::{Seq, SequenceCode, TaskStamp, Witness};

/// The operator's chosen disposition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DecisionChoice {
    /// Resolve every candidate output independently.
    #[default]
    Selected,
    /// Collapse the Design because this version's premise did not hold.
    Collapse,
}

/// One decision command and the authority heads its preview observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub request: PreparationDecide,
    pub expected: Vec<Witness>,
}

/// Where the decision draft stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftPhase {
    Editing,
    Previewing(Submission),
    Pending(Submission),
}

/// Last local or resident outcome, never projected authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftNotice {
    Refused(String),
    Recorded(String),
}

/// Browser-local decision state for one exact candidate version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionDraft {
    candidate: CandidateRef,
    choice: DecisionChoice,
    accepted: BTreeSet<TaskStamp>,
    rejected: BTreeSet<TaskStamp>,
    output_comments: BTreeMap<TaskStamp, String>,
    reason: String,
    phase: DraftPhase,
    notice: Option<DraftNotice>,
}

/// Browser-local decision drafts keyed by exact candidate version.
///
/// Settled drafts remain beside their versions while the operator compares or
/// edits another proposal. At most one draft owns the pending slot, and only
/// that exact candidate's response may settle it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionComposer {
    drafts: BTreeMap<CandidateRef, DecisionDraft>,
    pending: Option<CandidateRef>,
}

impl DecisionComposer {
    #[must_use]
    pub fn draft(&self, candidate: &CandidateRef) -> Option<&DecisionDraft> {
        self.drafts.get(candidate)
    }

    #[must_use]
    pub const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Apply one edit to its exact candidate without displacing the one
    /// in-flight draft.
    pub fn apply(
        &mut self,
        candidate: CandidateRef,
        settles: bool,
        change: impl FnOnce(&mut DecisionDraft),
    ) {
        let owns_pending = self.pending.as_ref() == Some(&candidate);
        if (settles && !owns_pending) || (self.pending.is_some() && !owns_pending) {
            return;
        }
        let draft = match self.drafts.entry(candidate) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let candidate = entry.key().clone();
                entry.insert(DecisionDraft::new(candidate))
            }
        };
        change(draft);
        if draft.is_pending() && self.pending.is_none() {
            self.pending = Some(draft.candidate().clone());
        } else if owns_pending && !draft.is_pending() {
            self.pending = None;
        }
    }
}

impl DecisionDraft {
    #[must_use]
    pub const fn new(candidate: CandidateRef) -> Self {
        Self {
            candidate,
            choice: DecisionChoice::Selected,
            accepted: BTreeSet::new(),
            rejected: BTreeSet::new(),
            output_comments: BTreeMap::new(),
            reason: String::new(),
            phase: DraftPhase::Editing,
            notice: None,
        }
    }

    #[must_use]
    pub const fn candidate(&self) -> &CandidateRef {
        &self.candidate
    }

    #[must_use]
    pub const fn choice(&self) -> DecisionChoice {
        self.choice
    }

    #[must_use]
    pub fn verdict(&self, output: &TaskStamp) -> Option<OutputVerdict> {
        if self.accepted.contains(output) {
            Some(OutputVerdict::Accepted)
        } else if self.rejected.contains(output) {
            Some(OutputVerdict::Rejected)
        } else {
            None
        }
    }

    #[must_use]
    pub fn output_comment(&self, output: &TaskStamp) -> &str {
        self.output_comments.get(output).map_or("", String::as_str)
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    #[must_use]
    pub const fn phase(&self) -> &DraftPhase {
        &self.phase
    }

    #[must_use]
    pub const fn notice(&self) -> Option<&DraftNotice> {
        self.notice.as_ref()
    }

    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self.phase, DraftPhase::Pending(_))
    }

    pub fn set_choice(&mut self, choice: DecisionChoice) {
        self.edit(|draft| draft.choice = choice);
    }

    pub fn set_output_verdict(&mut self, output: TaskStamp, verdict: OutputVerdict) {
        self.edit(|draft| match verdict {
            OutputVerdict::Accepted => {
                draft.rejected.remove(&output);
                draft.accepted.insert(output);
            }
            OutputVerdict::Rejected => {
                draft.accepted.remove(&output);
                draft.rejected.insert(output);
            }
        });
    }

    pub fn set_output_comment(&mut self, output: TaskStamp, comment: String) {
        self.edit(|draft| {
            draft.output_comments.insert(output, comment);
        });
    }

    pub fn set_reason(&mut self, reason: String) {
        self.edit(|draft| draft.reason = reason);
    }

    /// Preview against the exact candidate fact and current board heads.
    pub fn preview(
        &mut self,
        design: &BoardTask,
        candidate_position: Seq,
        candidate: &RetainedCandidate,
        next_decision: SequenceCode,
        tasks: &[BoardTask],
    ) {
        if self.is_pending() {
            return;
        }
        match self.submission(design, candidate_position, candidate, next_decision, tasks) {
            Ok(submission) => {
                self.phase = DraftPhase::Previewing(submission);
                self.notice = None;
            }
            Err(reason) => {
                self.phase = DraftPhase::Editing;
                self.notice = Some(DraftNotice::Refused(reason));
            }
        }
    }

    /// Take the preview once. The caller's in-flight guard closes the event-loop
    /// gap before this pending state redraws.
    pub fn begin_submit(&mut self) -> Option<Submission> {
        if !matches!(self.phase, DraftPhase::Previewing(_)) {
            return None;
        }
        let DraftPhase::Previewing(submission) =
            std::mem::replace(&mut self.phase, DraftPhase::Editing)
        else {
            unreachable!("checked previewing above");
        };
        self.phase = DraftPhase::Pending(submission.clone());
        self.notice = None;
        Some(submission)
    }

    /// Leave preview mode without changing the drafted verdicts or comments.
    #[cfg(target_arch = "wasm32")]
    pub fn revise(&mut self) {
        self.edit(|_| {});
    }

    /// Settle only an in-flight request. Success leaves standing to committed
    /// room authority; refusal keeps every choice for revision and re-preview.
    pub fn settle(&mut self, outcome: Result<TextAnswer, String>) {
        if !self.is_pending() {
            return;
        }
        self.phase = DraftPhase::Editing;
        self.notice = Some(match outcome {
            Ok(answer) => DraftNotice::Recorded(answer.text),
            Err(reason) => DraftNotice::Refused(reason),
        });
    }

    fn edit(&mut self, change: impl FnOnce(&mut Self)) {
        if self.is_pending() {
            return;
        }
        self.phase = DraftPhase::Editing;
        self.notice = None;
        change(self);
    }

    fn submission(
        &self,
        design: &BoardTask,
        candidate_position: Seq,
        candidate: &RetainedCandidate,
        next_decision: SequenceCode,
        tasks: &[BoardTask],
    ) -> Result<Submission, String> {
        let design_stamp = &design.statement.body.born;
        if candidate.candidate != self.candidate || candidate.candidate.controller != *design_stamp
        {
            return Err(
                "preparation::candidate-mismatch: preview the exact proposal version this draft names"
                    .to_owned(),
            );
        }
        if !design
            .design_standing()
            .is_some_and(|standing| standing.is_open())
        {
            return Err("preparation::design-not-open: decide an open Design".to_owned());
        }

        let outcome = self.decision_outcome(candidate)?;
        let decision = DecisionRef {
            controller: design_stamp.clone(),
            decision: next_decision,
        };
        let expected = decision_witnesses(
            design,
            candidate_position,
            candidate,
            &decision,
            &outcome,
            tasks,
        )?;
        Ok(Submission {
            request: PreparationDecide {
                decision: PreparationDecision {
                    decision,
                    source: RoomSource::Journal {
                        fact: SequenceCode::new(candidate_position.get()),
                    },
                    outcome,
                },
            },
            expected,
        })
    }

    fn decision_outcome(&self, candidate: &RetainedCandidate) -> Result<DecisionOutcome, String> {
        match self.choice {
            DecisionChoice::Selected => {
                if self
                    .accepted
                    .iter()
                    .chain(&self.rejected)
                    .any(|output| !candidate.outputs.contains(output))
                {
                    return Err(
                        "preparation::decision-foreign-output: decide only outputs in this proposal version"
                            .to_owned(),
                    );
                }
                let outputs = candidate
                    .outputs
                    .iter()
                    .map(|output| {
                        let verdict = self.verdict(output).ok_or_else(|| {
                            format!(
                                "preparation::output-undecided: accept or reject output {output}"
                            )
                        })?;
                        let comment = trimmed(self.output_comment(output));
                        Ok(OutputDecision {
                            output: output.clone(),
                            verdict,
                            comment,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(DecisionOutcome::Resolved {
                    candidate: candidate.candidate.clone(),
                    outputs,
                    comment: trimmed(&self.reason),
                })
            }
            DecisionChoice::Collapse => {
                let reason = self.reason.trim();
                if reason.is_empty() {
                    return Err(
                        "preparation::collapse-reason-empty: state why the Design premise did not hold"
                            .to_owned(),
                    );
                }
                Ok(DecisionOutcome::Collapse {
                    candidate: candidate.candidate.clone(),
                    reason: reason.to_owned(),
                })
            }
        }
    }
}

fn trimmed(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn decision_witnesses(
    design: &BoardTask,
    candidate_position: Seq,
    candidate: &RetainedCandidate,
    decision: &DecisionRef,
    outcome: &DecisionOutcome,
    tasks: &[BoardTask],
) -> Result<Vec<Witness>, String> {
    let mut expected = transition_witnesses(design);
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Decision {
            decision: decision.clone(),
        },
        head: None,
    });
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: candidate.candidate.clone(),
        },
        head: Some(candidate_position),
    });
    if outcome.accepted_outputs().next().is_none() {
        return Ok(expected);
    }
    for input in &candidate.governed_inputs {
        expected.push(statement_witness(find_task(tasks, input).ok_or_else(
            || {
                format!(
                    "preparation::input-unavailable: governed input {input} is not on the board"
                )
            },
        )?));
    }
    for output in outcome.accepted_outputs() {
        let task = find_task(tasks, output).ok_or_else(|| {
            format!("preparation::output-unavailable: accepted output {output} is not on the board")
        })?;
        expected.extend(transition_witnesses(task));
        expected.push(Witness::Authorization {
            output: output.clone(),
            head: None,
        });
    }
    Ok(expected)
}

fn find_task<'a>(tasks: &'a [BoardTask], stamp: &TaskStamp) -> Option<&'a BoardTask> {
    tasks.iter().find(|task| task.statement.body.born == *stamp)
}

fn statement_witness(task: &BoardTask) -> Witness {
    Witness::Task {
        task: task.statement.body.born.clone(),
        head: task.statement.position,
    }
}

fn transition_witnesses(task: &BoardTask) -> Vec<Witness> {
    let stamp = &task.statement.body.born;
    vec![
        statement_witness(task),
        Witness::Workflow {
            task: stamp.clone(),
            head: task.workflow.position,
            phase: task.workflow.body.phase.clone(),
        },
        Witness::Claim {
            task: stamp.clone(),
            head: task.claim.as_ref().map(|claim| claim.position),
        },
    ]
}

#[cfg(test)]
mod tests;
