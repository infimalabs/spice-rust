//! Replay of the claim position governing each Design pass.
//!
//! A Design pass names its controller's claim-head position. Winning,
//! carrying, rebinding, and explicit generation acknowledgement set it; the
//! resident's lease timer never moves it. Retained manual renewals also moved
//! that position when they acknowledged a generation. Replay follows the
//! commit guard's acknowledgement rule (`guard/generation.rs`) from facts
//! alone: a transaction mutating the task leaves its claim trailing unless
//! that transaction won the claim or the holder's own actor committed it.

use std::collections::{BTreeMap, BTreeSet};

use wire::{Actor, AnyFact, FactKind, Instant, TaskStamp};

/// Whether each active claim acknowledged its task's latest generation.
#[derive(Default)]
pub(super) struct Acknowledgements {
    claims: BTreeMap<String, Acknowledgement>,
    /// Writer and commit instant shared by the facts of the transaction being
    /// replayed, beside the tasks whose claim that transaction won.
    transaction: Option<(Actor, Instant)>,
    won: BTreeSet<String>,
}

struct Acknowledgement {
    holder: Actor,
    current: bool,
}

impl Acknowledgements {
    /// Observe one fact in history order, answering the task whose claim head
    /// a retained manual renewal rewrote at this fact.
    pub(super) fn observe(
        &mut self,
        actor: &Actor,
        at: Instant,
        fact: &AnyFact,
    ) -> Option<TaskStamp> {
        let transaction = (actor.clone(), at);
        if self.transaction.as_ref() != Some(&transaction) {
            self.transaction = Some(transaction);
            self.won.clear();
        }
        if let Some(won) = fact.read::<wire::body::ClaimWon>() {
            self.won.insert(won.task.to_string());
            self.claims.insert(
                won.task.to_string(),
                Acknowledgement {
                    holder: won.actor,
                    current: true,
                },
            );
            return None;
        }
        if let Some(rebound) = fact.read::<wire::body::ClaimRebound>() {
            if let Some(claim) = self.claims.get_mut(rebound.task.as_str()) {
                claim.current = true;
            }
            return None;
        }
        if let Some(acknowledged) = fact.read::<wire::body::ClaimAcknowledged>() {
            if let Some(claim) = self.claims.get_mut(acknowledged.task.as_str()) {
                claim.current = true;
            }
            return None;
        }
        if let Some(renewed) = fact.read::<wire::body::ClaimRenewed>() {
            let claim = self.claims.get_mut(renewed.task.as_str())?;
            if *actor == Actor::Spice || claim.current {
                return None;
            }
            claim.current = true;
            return Some(renewed.task);
        }
        if matches!(fact.kind(), FactKind::ClaimFreed | FactKind::ClaimLapsed)
            && let Some(task) = fact.task_subject()
        {
            self.claims.remove(task.as_str());
            return None;
        }
        // A retained task note stays the historical change to its task it was.
        let retained = wire::body::RetainedTaskNote::read(fact).map(|note| note.task);
        for task in fact.mutated_tasks().chain(retained.as_ref()) {
            if self.won.contains(task.as_str()) {
                continue;
            }
            if let Some(claim) = self.claims.get_mut(task.as_str())
                && claim.holder != *actor
            {
                claim.current = false;
            }
        }
        None
    }
}
