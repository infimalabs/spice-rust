//! Full-history replay authority folded from immutable facts alone.

use std::collections::BTreeMap;

use wire::rooms::{DesignProposed, PreparationDecision};
use wire::{
    Actor, AnyFact, CanonicalUuid, Instant, Seq, TaskPreparation, TaskStamp, TranscriptSourceHead,
};

use super::{
    Authority, Change, Projection, Refusal, claims, conversation, dependencies, fold, relevance,
};

/// Every task, transcript generation, proposal author, and operator direction
/// the history has introduced, beside the projection it derives.
#[derive(Default)]
pub(crate) struct History {
    projection: Projection,
    tasks: BTreeMap<String, Option<TaskPreparation>>,
    task_intents: BTreeMap<String, Option<wire::TaskIntent>>,
    dependencies: dependencies::ReplayDependencies,
    task_statements: BTreeMap<String, Seq>,
    active_claims: BTreeMap<String, Seq>,
    acknowledgements: claims::Acknowledgements,
    transcripts: BTreeMap<String, TranscriptSourceHead>,
    /// Proposal head position to the actor that recorded it.
    authors: BTreeMap<u64, Actor>,
    /// Operator direction key to the lane it addresses.
    operator_directions: BTreeMap<String, CanonicalUuid>,
}

impl History {
    /// Fold the complete canonical history, collecting every refusal by
    /// position instead of stopping at the first.
    pub(crate) fn rebuild<'a>(
        facts: impl IntoIterator<Item = (Seq, &'a Actor, Instant, &'a AnyFact)>,
    ) -> (Projection, Vec<(Seq, Refusal)>) {
        let mut history = Self::default();
        let mut refusals = Vec::new();
        for (seq, actor, at, fact) in facts {
            if let Some(task) = history.acknowledgements.observe(actor, at, fact) {
                history.set_active_claim(&task, Some(seq));
            }
            // A proposal's author decides whether later coverage is
            // independent of it.
            if fact.read::<DesignProposed>().is_some() {
                history.authors.insert(seq.get(), actor.clone());
            }
            if let Err(refusal) = fold(&mut history, seq, at, actor, fact) {
                refusals.push((seq, refusal));
            }
        }
        (history.projection, refusals)
    }
}

impl Authority for History {
    fn task(&self, task: &TaskStamp) -> Result<Option<Option<TaskPreparation>>, Refusal> {
        Ok(self.tasks.get(task.as_str()).cloned())
    }

    fn set_task(&mut self, task: &TaskStamp, preparation: Option<TaskPreparation>) {
        self.tasks.insert(task.as_str().to_owned(), preparation);
    }
    fn task_intent(&self, task: &TaskStamp) -> Result<Option<Option<wire::TaskIntent>>, Refusal> {
        Ok(self.task_intents.get(task.as_str()).cloned())
    }

    fn set_task_intent(&mut self, task: &TaskStamp, intent: Option<wire::TaskIntent>) {
        self.task_intents.insert(task.to_string(), intent);
    }
    fn task_dependencies(&self, task: &TaskStamp) -> Result<Option<Vec<TaskStamp>>, Refusal> {
        Ok(self.dependencies.get(task))
    }

    fn set_task_dependencies(&mut self, task: &TaskStamp, dependencies: Vec<TaskStamp>) {
        self.dependencies.set(task, dependencies);
    }
    fn set_task_birth_dependencies(&mut self, added: &wire::body::TaskAdded) {
        self.dependencies.birth(added);
    }
    fn set_task_edit_dependencies(&mut self, task: &TaskStamp, dependencies: Vec<TaskStamp>) {
        self.dependencies.edit(task, dependencies);
    }
    fn task_statement(&self, task: &TaskStamp) -> Result<Option<Seq>, Refusal> {
        Ok(self.task_statements.get(task.as_str()).copied())
    }

    fn set_task_statement(&mut self, task: &TaskStamp, statement: Seq) {
        self.task_statements
            .insert(task.as_str().to_owned(), statement);
    }

    fn active_claim(&self, task: &TaskStamp) -> Result<Option<Seq>, Refusal> {
        Ok(self.active_claims.get(task.as_str()).copied())
    }

    fn set_active_claim(&mut self, task: &TaskStamp, claim: Option<Seq>) {
        match claim {
            Some(claim) => {
                self.active_claims.insert(task.to_string(), claim);
            }
            None => {
                self.active_claims.remove(task.as_str());
            }
        }
    }

    fn account(&mut self, head: &TranscriptSourceHead) {
        self.transcripts
            .insert(head.key().into_string(), head.clone());
    }

    fn transcript(
        &self,
        head: &TranscriptSourceHead,
    ) -> Result<Option<TranscriptSourceHead>, Refusal> {
        Ok(self.transcripts.get(head.key().as_str()).cloned())
    }

    fn published(&self, room: &str, publication: &str) -> Result<bool, Refusal> {
        Ok(self
            .projection
            .publications
            .contains_key(&(room.to_owned(), publication.to_owned())))
    }
    fn publication_at(&self, seq: u64) -> Result<bool, Refusal> {
        Ok(self
            .projection
            .publications
            .values()
            .any(|published| *published == seq))
    }
    fn request_at(&self, seq: u64) -> Result<bool, Refusal> {
        Ok(self
            .projection
            .notices
            .keys()
            .any(|(_, noticed)| *noticed == seq))
    }

    fn cursor(&self, lane: &str) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .projection
            .cursors
            .get(lane)
            .map(|(_, body)| body.clone()))
    }

    fn horizon(&self, lane: &str) -> Result<bool, Refusal> {
        Ok(self.projection.horizons.contains_key(lane))
    }

    fn group_partition(&self) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .projection
            .partition
            .as_ref()
            .map(|(_, body)| body.clone()))
    }
    fn oops_generation(&self) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .projection
            .oops_generation
            .as_ref()
            .map(|(_, body)| body.clone()))
    }

    fn proposal(&self, subject: &str) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .projection
            .proposals
            .get(subject)
            .map(|row| row.body.clone()))
    }

    fn proposal_author(&self, subject: &str) -> Result<Option<Actor>, Refusal> {
        Ok(self
            .projection
            .proposals
            .get(subject)
            .and_then(|row| self.authors.get(&row.seq))
            .cloned())
    }

    fn candidate_decided(&self, candidate: &wire::rooms::CandidateRef) -> Result<bool, Refusal> {
        for row in self.projection.proposals.values() {
            if let Ok(decision) = wire::decode::<PreparationDecision>(&row.body)
                && decision.outcome.candidate() == candidate
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn decision_authorized(&self, decision: &str) -> Result<bool, Refusal> {
        Ok(self
            .projection
            .authorizations
            .values()
            .any(|row| row.decision == decision))
    }

    fn operator_direction(&self, key: &str) -> Result<Option<CanonicalUuid>, Refusal> {
        Ok(self.operator_directions.get(key).copied())
    }

    fn set_operator_direction(&mut self, key: &str, lane: CanonicalUuid) {
        self.operator_directions.insert(key.to_owned(), lane);
    }

    fn conversation(&self, publication: &str) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .projection
            .conversation
            .publications
            .get(publication)
            .map(|(_, body)| body.clone()))
    }

    fn room_item(&self, item: &str) -> Result<Option<conversation::ItemRow>, Refusal> {
        Ok(self.projection.conversation.items.get(item).cloned())
    }

    fn relevance_observation(
        &self,
        observation: &str,
    ) -> Result<Option<relevance::ObservationRow>, Refusal> {
        Ok(self
            .projection
            .relevance
            .observations
            .get(observation)
            .cloned())
    }

    fn relevance_assessment(
        &self,
        observation: &str,
    ) -> Result<Option<relevance::AssessmentRow>, Refusal> {
        Ok(self
            .projection
            .relevance
            .assessments
            .get(observation)
            .cloned())
    }

    fn change(&mut self, change: Change) -> Result<(), Refusal> {
        self.projection.apply(change)
    }
}
