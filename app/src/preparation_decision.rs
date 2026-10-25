//! One operator decision over one immutable Design pass proposal: resolve each
//! work output, accept an outputless conclusion, or return any proposal with
//! material feedback; preview the effect; then submit once.
//!
//! No output verdict is assumed: every proposed output stays undecided until
//! the operator marks it, singly or all at once, and a work proposal with an
//! undecided output refuses to preview rather than deciding for them. No
//! decision kind is chosen either: entered return feedback makes the decision
//! a return, and otherwise it accepts the proposal through its verdicts.
//!
//! The draft never changes rendered authority. A successful command answer
//! only reports that the write landed; proposal standing moves when the
//! committed decision and refreshed board dependency edges arrive.

use std::collections::{BTreeMap, btree_map::Entry};

use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateRef, DecisionOutcome, DecisionRef, DesignPassProposal, DesignPassRecommendation,
    OutputDecision, OutputVerdict, PreparationDecide, PreparationDecision, ProposalRef, RoomSource,
};
use wire::{Seq, SequenceCode, TaskStamp, Witness};

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
    comment: String,
    feedback: String,
    verdicts: BTreeMap<TaskStamp, OutputVerdict>,
    output_comments: BTreeMap<TaskStamp, String>,
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
            comment: String::new(),
            feedback: String::new(),
            verdicts: BTreeMap::new(),
            output_comments: BTreeMap::new(),
            phase: DraftPhase::Editing,
            notice: None,
        }
    }

    #[must_use]
    pub const fn candidate(&self) -> &CandidateRef {
        &self.candidate
    }

    /// Whether return feedback is entered, which makes the decision a return.
    #[cfg(target_arch = "wasm32")]
    #[must_use]
    pub fn returning(&self) -> bool {
        !self.feedback.trim().is_empty()
    }

    /// The verdict the operator gave `output`, if any.
    #[must_use]
    pub fn verdict(&self, output: &TaskStamp) -> Option<OutputVerdict> {
        self.verdicts.get(output).copied()
    }

    #[must_use]
    pub fn output_comment(&self, output: &TaskStamp) -> &str {
        self.output_comments.get(output).map_or("", String::as_str)
    }

    /// The overall comment an accepted work proposal records.
    #[cfg(target_arch = "wasm32")]
    #[must_use]
    pub fn comment(&self) -> &str {
        &self.comment
    }

    /// The material feedback that returns the proposal when entered.
    #[must_use]
    pub fn feedback(&self) -> &str {
        &self.feedback
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

    pub fn set_output_verdict(&mut self, output: TaskStamp, verdict: OutputVerdict) {
        self.edit(|draft| {
            draft.verdicts.insert(output, verdict);
        });
    }

    /// Give every listed output the same verdict in one edit.
    pub fn set_every_output_verdict(
        &mut self,
        outputs: impl IntoIterator<Item = TaskStamp>,
        verdict: OutputVerdict,
    ) {
        self.edit(|draft| {
            for output in outputs {
                draft.verdicts.insert(output, verdict);
            }
        });
    }

    pub fn set_output_comment(&mut self, output: TaskStamp, comment: String) {
        self.edit(|draft| {
            draft.output_comments.insert(output, comment);
        });
    }

    pub fn set_comment(&mut self, comment: String) {
        self.edit(|draft| draft.comment = comment);
    }

    pub fn set_feedback(&mut self, feedback: String) {
        self.edit(|draft| draft.feedback = feedback);
    }

    /// Preview against the exact candidate fact and current board heads.
    pub fn preview(
        &mut self,
        design: &BoardTask,
        candidate_position: Seq,
        candidate: &DesignPassProposal,
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
        candidate: &DesignPassProposal,
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

    fn decision_outcome(&self, candidate: &DesignPassProposal) -> Result<DecisionOutcome, String> {
        let feedback = self.feedback.trim();
        if !feedback.is_empty() {
            return Ok(DecisionOutcome::Returned {
                candidate: candidate.candidate.clone(),
                reason: feedback.to_owned(),
            });
        }
        match candidate.recommendation {
            DesignPassRecommendation::Work => {
                if self
                    .verdicts
                    .keys()
                    .any(|output| !candidate.outputs.contains(output))
                {
                    return Err(
                        "preparation::decision-foreign-output: decide only outputs in this proposal version"
                            .to_owned(),
                    );
                }
                let undecided = candidate
                    .outputs
                    .iter()
                    .filter(|output| !self.verdicts.contains_key(*output))
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                if !undecided.is_empty() {
                    return Err(format!(
                        "preparation::decision-undecided-output: accept or reject every proposed output before previewing; undecided: {}",
                        undecided.join(", ")
                    ));
                }
                let outputs = candidate
                    .outputs
                    .iter()
                    .map(|output| OutputDecision {
                        output: output.clone(),
                        verdict: self.verdicts[output],
                        comment: trimmed(self.output_comment(output)),
                    })
                    .collect();
                Ok(DecisionOutcome::Resolved {
                    candidate: candidate.candidate.clone(),
                    outputs,
                    comment: trimmed(&self.comment),
                })
            }
            DesignPassRecommendation::Conclude | DesignPassRecommendation::Collapse => {
                Ok(DecisionOutcome::Accepted {
                    candidate: candidate.candidate.clone(),
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
    candidate: &DesignPassProposal,
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
