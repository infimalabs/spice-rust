//! Source-qualified transcript sessions, reconstructed once and advanced by delta.

use std::collections::BTreeMap;
use std::path::PathBuf;

use journal::crossing::{AnyFact, CanonicalUuid, Envelope, FactKind, Seq, body};
use journal::{FactReadBudget, FactSnapshot, GuardError, Journal};

type Position = (u64, u64);
type Stream = (CanonicalUuid, String, String);
type Sessions = BTreeMap<(Position, Seq), TranscriptSession>;
type PendingSessions = BTreeMap<Position, Vec<(Seq, TranscriptSession)>>;

/// Author and worktree authenticated by a source-qualified session declaration.
#[derive(Clone, Debug)]
pub struct TranscriptSession {
    /// Occupant that authored this session.
    pub occupant: CanonicalUuid,
    /// Vendor whose dialect decodes this session.
    pub vendor: String,
    /// Worktree bound to this session.
    pub worktree: PathBuf,
}

/// Disposable session lookup, with declarations awaiting their first ingestion.
#[derive(Clone, Debug, Default)]
pub struct TranscriptSessions {
    sources: BTreeMap<(CanonicalUuid, CanonicalUuid), Sessions>,
    pending: BTreeMap<Stream, PendingSessions>,
}

#[cfg(test)]
mod tests;

impl TranscriptSessions {
    /// Recover session bindings in journal order without rescanning ingestions.
    #[must_use]
    pub fn from_snapshot(snapshot: &FactSnapshot) -> Self {
        let mut sessions = Self::default();
        for fact in snapshot.facts() {
            sessions.apply(fact);
        }
        sessions
    }

    fn apply(&mut self, envelope: &Envelope<AnyFact>) {
        if let Some(session) = envelope.fact.read::<body::LaneSessionDeclared>() {
            self.pending
                .entry((session.lane, session.vendor.to_string(), session.stream))
                .or_default()
                .entry((session.member, session.offset))
                .or_default()
                .push((
                    envelope.seq,
                    TranscriptSession {
                        occupant: session.occupant,
                        vendor: session.vendor.to_string(),
                        worktree: session.worktree.into(),
                    },
                ));
        } else if let Some(batch) = envelope.fact.read::<body::JournalIngested>() {
            let stream = (batch.lane, batch.vendor.to_string(), batch.stream);
            let Some(pending) = self.pending.get_mut(&stream) else {
                return;
            };
            let from = (batch.from.member, batch.from.offset);
            let to = (batch.to.member, batch.to.offset);
            if from >= to {
                return;
            }
            // Only declarations preceding this batch can bind to its source.
            // Remove each match so a later overlapping batch cannot rebind it.
            while let Some((&position, _)) = pending.range(from..to).next() {
                let declarations = pending
                    .remove(&position)
                    .expect("observed pending position");
                let sessions = self.sources.entry((batch.lane, batch.source)).or_default();
                for (seq, session) in declarations {
                    sessions.insert((position, seq), session);
                }
            }
            if pending.is_empty() {
                self.pending.remove(&stream);
            }
        }
    }

    /// Latest session at or before one position in an exact source generation.
    #[must_use]
    pub fn at(
        &self,
        lane: CanonicalUuid,
        source: CanonicalUuid,
        member: u64,
        offset: u64,
    ) -> Option<&TranscriptSession> {
        self.sources
            .get(&(lane, source))?
            .range(
                ..=(
                    (member, offset),
                    Seq::new(u64::MAX).expect("nonzero sequence"),
                ),
            )
            .next_back()
            .map(|(_, session)| session)
    }
}

/// Owned by one writer generation; successful pages advance the recovery cursor.
#[derive(Default)]
pub(super) struct TranscriptSessionState {
    sessions: TranscriptSessions,
    cursor: Option<Seq>,
}

impl TranscriptSessionState {
    pub(super) fn lookup(
        &mut self,
        journal: &Journal,
        lane: CanonicalUuid,
        source: CanonicalUuid,
        member: u64,
        offset: u64,
    ) -> Result<Option<TranscriptSession>, GuardError> {
        journal.read(|read| {
            while self.cursor != read.cursor()? {
                let page = read.selected_fact_page(
                    &[FactKind::LaneSessionDeclared, FactKind::JournalIngested],
                    self.cursor,
                    FactReadBudget::RETAINED,
                )?;
                for fact in &page.rows {
                    self.sessions.apply(fact);
                }
                self.cursor = page.continuation.or(page.horizon);
            }
            Ok(self.sessions.at(lane, source, member, offset).cloned())
        })
    }
}
