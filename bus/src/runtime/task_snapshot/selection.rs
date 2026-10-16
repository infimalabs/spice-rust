//! Candidate-version precedence for selection waiting, advanced once per
//! commit so a board read classifies each output with one lookup.

use std::collections::HashMap;
use std::sync::Arc;

use journal::crossing::TaskStamp;
use journal::crossing::rooms::{CandidateRef, DecisionRef, RetainedCandidate};

use super::persistent_index::PersistentIndex;

/// Selection standing of one retained candidate version.
#[derive(Clone, Debug, Eq, PartialEq)]
enum CandidateStanding {
    /// Retained and still awaiting the operator's decision.
    Open,
    /// Ruled on by this decision.
    Decided(DecisionRef),
    /// Withdrawn by its author before any decision.
    Withdrawn,
}

/// One retained candidate version: the outputs it names and its standing.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RetainedVersion {
    outputs: Arc<[TaskStamp]>,
    standing: CandidateStanding,
}

/// Every retained candidate version and the decision classifying each output
/// it names: the verdict on the latest unwithdrawn version naming that output,
/// in journal order. A later open version shadows an older verdict without
/// rewriting it, and withdrawing that version restores the verdict beneath.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct CandidateSelection {
    versions: PersistentIndex<CandidateRef, RetainedVersion>,
    /// Candidate versions naming each output, in retention order.
    naming: PersistentIndex<TaskStamp, Arc<[CandidateRef]>>,
    /// `None` is a recorded entry: a later open version shadows a verdict.
    decisions: PersistentIndex<TaskStamp, Option<DecisionRef>>,
}

impl CandidateSelection {
    /// Record a new open candidate version; it shadows any older verdict on
    /// each output it names until it is decided or withdrawn.
    pub(super) fn retain(&mut self, retained: RetainedCandidate) {
        if self.versions.get(&retained.candidate).is_some() {
            return;
        }
        let outputs: Arc<[TaskStamp]> = retained.outputs.into();
        let mut naming = HashMap::with_capacity(outputs.len());
        let mut decisions = HashMap::with_capacity(outputs.len());
        for output in outputs.iter() {
            let mut named = self
                .naming
                .get(output)
                .map_or_else(Vec::new, |named| named.to_vec());
            named.push(retained.candidate.clone());
            naming.insert(output.clone(), Arc::from(named));
            decisions.insert(output.clone(), None);
        }
        self.versions = self.versions.pushed(HashMap::from([(
            retained.candidate,
            RetainedVersion {
                outputs,
                standing: CandidateStanding::Open,
            },
        )]));
        self.naming = self.naming.pushed(naming);
        self.decisions = self.decisions.pushed(decisions);
    }

    /// Record the operator's decision on one candidate version.
    pub(super) fn decide(&mut self, candidate: &CandidateRef, decision: DecisionRef) {
        self.settle(candidate, CandidateStanding::Decided(decision));
    }

    /// Record the author's withdrawal of one candidate version.
    pub(super) fn withdraw(&mut self, candidate: &CandidateRef) {
        self.settle(candidate, CandidateStanding::Withdrawn);
    }

    /// The decision classifying one output; `None` while the latest
    /// unwithdrawn version naming it is open or no version ruled on it.
    pub(super) fn decision(&self, output: &TaskStamp) -> Option<&DecisionRef> {
        self.decisions.get(output)?.as_ref()
    }

    /// Settle one candidate version, then reclassify each output it names.
    fn settle(&mut self, candidate: &CandidateRef, standing: CandidateStanding) {
        let Some(version) = self.versions.get(candidate) else {
            return;
        };
        let outputs = Arc::clone(&version.outputs);
        self.versions = self.versions.pushed(HashMap::from([(
            candidate.clone(),
            RetainedVersion {
                outputs: Arc::clone(&outputs),
                standing,
            },
        )]));
        let decisions = outputs
            .iter()
            .map(|output| (output.clone(), self.classify(output)))
            .collect();
        self.decisions = self.decisions.pushed(decisions);
    }

    /// The verdict on the latest unwithdrawn version naming one output.
    fn classify(&self, output: &TaskStamp) -> Option<DecisionRef> {
        let named = self.naming.get(output)?;
        named
            .iter()
            .rev()
            .find_map(|candidate| match &self.versions.get(candidate)?.standing {
                CandidateStanding::Withdrawn => None,
                CandidateStanding::Open => Some(None),
                CandidateStanding::Decided(decision) => Some(Some(decision.clone())),
            })?
    }
}
