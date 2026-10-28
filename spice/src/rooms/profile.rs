//! Evidence ownership for continuing vendor sessions.
//!
//! A logical lane can host many unrelated conversations, and one conversation
//! can resume under several occupants or tape generations. [`SessionKey`]
//! therefore uses only the lane, vendor, and vendor-declared resumable identity.
//! Occupants are lookup witnesses; they are never profile identities.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use board::similarity::Background;
use wire::{CanonicalUuid, Name, Seq, TaskStamp, body::LaneSessionDeclared};

use super::evidence::{Digest, Evidence, EvidenceProfile, Kind};

/// Stable owner of one continuing conversation's evidence.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionKey {
    /// Logical lane in which the vendor conversation runs.
    pub lane: CanonicalUuid,
    /// Vendor whose declaration authenticated the identity.
    pub vendor: Name,
    /// Exact resumable identity admitted from the selected vendor stream.
    pub identity: String,
}

impl SessionKey {
    /// Build a key from already-admitted session authority.
    #[must_use]
    pub fn new(lane: CanonicalUuid, vendor: Name, identity: impl Into<String>) -> Self {
        Self {
            lane,
            vendor,
            identity: identity.into(),
        }
    }

    /// Key one admitted declaration establishes.
    #[must_use]
    pub fn declared(declaration: &LaneSessionDeclared) -> Self {
        Self::new(
            declaration.lane,
            declaration.vendor.clone(),
            declaration.identity.clone(),
        )
    }
}

/// Inspectable source of one retained evidence card.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EvidenceKey {
    /// Exact statement revision and successful completion observed together,
    /// classified by the phase the completed claim held.
    Task {
        task: TaskStamp,
        statement: Seq,
        completion: Seq,
        support: wire::rooms::DesignSupport,
    },
    /// Exact admitted transcript bytes carrying settled assistant prose.
    Conversation {
        source: CanonicalUuid,
        member: u64,
        offset: u64,
        length: u64,
    },
}

/// Oldest transcript position already crossed by a bounded warm scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TranscriptCursor {
    pub source: CanonicalUuid,
    pub member: u64,
    pub offset: u64,
}

/// Independent durable-history continuations for one warm operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WarmCursor {
    /// Completion fact the next page resumes strictly before; `None` begins at
    /// the newest completion.
    pub facts: Option<Seq>,
    /// Oldest transcript position already crossed.
    pub transcript: Option<TranscriptCursor>,
    /// Whether completion history reached its beginning or the task class is
    /// already full of newer cards.
    pub facts_complete: bool,
    /// Whether transcript history reached its beginning or the conversation
    /// class is already full of newer cards.
    pub transcript_complete: bool,
}

/// Historical acquisition state of one profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WarmState {
    /// No historical acquisition is owed; an empty profile means empty history.
    Ready { through: WarmCursor },
    /// The first bounded historical page has not completed yet.
    Warming { cursor: WarmCursor },
    /// A bounded page completed, with more admitted history still owed.
    Incomplete { cursor: WarmCursor },
    /// Historical acquisition refused; retained live evidence remains visible.
    Failed { cursor: WarmCursor, detail: String },
}

impl Default for WarmState {
    fn default() -> Self {
        Self::Ready {
            through: WarmCursor {
                facts_complete: true,
                transcript_complete: true,
                ..WarmCursor::default()
            },
        }
    }
}

impl WarmState {
    /// Cursors after which the next bounded warm step resumes.
    #[must_use]
    pub const fn cursor(&self) -> WarmCursor {
        match self {
            Self::Ready { through } => *through,
            Self::Warming { cursor }
            | Self::Incomplete { cursor }
            | Self::Failed { cursor, .. } => *cursor,
        }
    }

    /// Whether historical acquisition completed successfully.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// One continuing session's evidence and acquisition standing.
#[derive(Debug, Default)]
pub struct SessionProfile {
    evidence: EvidenceProfile<EvidenceKey>,
    warm: WarmState,
}

impl SessionProfile {
    /// Historical acquisition standing. Callers must not interpret a non-ready
    /// profile with no cards as evidence that the session knows nothing.
    #[must_use]
    pub const fn warm_state(&self) -> &WarmState {
        &self.warm
    }

    /// Retained evidence, including any cards acquired before an incomplete or
    /// failed warm step.
    #[must_use]
    pub const fn evidence(&self) -> &EvidenceProfile<EvidenceKey> {
        &self.evidence
    }

    /// Mutable scoring access. Selection may inspect partial cards only when it
    /// also preserves [`Self::warm_state`] in its result semantics.
    #[must_use]
    pub fn evidence_mut(&mut self) -> &mut EvidenceProfile<EvidenceKey> {
        &mut self.evidence
    }

    /// Whether historical acquisition is still owed: neither ready nor failed.
    const fn owes(&self) -> bool {
        matches!(
            self.warm,
            WarmState::Warming { .. } | WarmState::Incomplete { .. }
        )
    }

    /// Adopt `cursor` after a successful step. A class already full of newer
    /// cards completes its source, since older history can no longer change
    /// what is retained. Returns the cursor when the profile just became ready.
    fn settle(&mut self, mut cursor: WarmCursor) -> Option<WarmCursor> {
        cursor.facts_complete |= self.evidence.is_full(Kind::Task);
        cursor.transcript_complete |= self.evidence.is_full(Kind::Conversation);
        let was_ready = self.warm.is_ready();
        self.warm = if cursor.facts_complete && cursor.transcript_complete {
            WarmState::Ready { through: cursor }
        } else {
            WarmState::Incomplete { cursor }
        };
        (!was_ready && self.warm.is_ready()).then_some(cursor)
    }
}

/// Result of attempting to retain one card for an occupant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Inserted {
    /// The occupant or session has no admitted vendor-session declaration.
    Undeclared,
    /// An exact retained content digest already represented this evidence.
    Duplicate,
    /// Older history found a card whose class is already full of newer cards.
    Full,
    /// The card was retained, evicting its class's oldest card if necessary.
    Stored,
}

/// Why a selector should reconsider one continuing session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileOpportunityKind {
    /// An occupant declared or resumed this exact session.
    Declared { occupant: CanonicalUuid },
    /// One previously unseen evidence card was retained.
    Stored { key: EvidenceKey, digest: Digest },
    /// Both bounded history sources reached their beginnings.
    Ready { through: WarmCursor },
    /// A Design statement changed and may now fit this session.
    DesignChanged { statement: Seq },
}

/// One process-local, cursor-addressable selection trigger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileOpportunity {
    /// Monotonic process-local cursor.
    pub cursor: u64,
    /// Continuing session to reconsider.
    pub session: SessionKey,
    /// Change that created this opportunity.
    pub kind: ProfileOpportunityKind,
}

/// One bounded opportunity page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileOpportunityPage {
    /// Opportunities after the requested cursor, oldest first.
    pub rows: Vec<ProfileOpportunity>,
    /// Cursor to pass to the next read.
    pub next: Option<u64>,
    /// True when the caller's cursor fell behind retained opportunities.
    pub missed: bool,
    /// True when another retained opportunity follows this page.
    pub more: bool,
}

/// Maximum process-local selection opportunities retained for resumption.
pub const PROFILE_OPPORTUNITIES: usize = 1024;

/// Resident-owned evidence profiles and occupant-to-session declarations.
#[derive(Debug, Default)]
pub struct SessionProfiles {
    occupants: BTreeMap<(CanonicalUuid, CanonicalUuid), SessionKey>,
    profiles: BTreeMap<SessionKey, SessionProfile>,
    opportunities: VecDeque<ProfileOpportunity>,
    next_opportunity: u64,
}

impl SessionProfiles {
    /// Admit an exact declaration and return the continuing-session key it
    /// establishes. A successor occupant declaring the same key sees the same
    /// profile; a different identity creates a different profile.
    pub fn declare(&mut self, declaration: &LaneSessionDeclared) -> SessionKey {
        let key = SessionKey::declared(declaration);
        self.bind(declaration.occupant, key.clone());
        key
    }

    /// Bind one admitted occupant to a stable session key.
    pub fn bind(&mut self, occupant: CanonicalUuid, key: SessionKey) {
        if self.occupants.get(&(key.lane, occupant)) == Some(&key) {
            return;
        }
        self.occupants.insert((key.lane, occupant), key.clone());
        self.profiles.entry(key.clone()).or_default();
        self.push_opportunity(key, ProfileOpportunityKind::Declared { occupant });
    }

    /// Forget one occupant lookup without deleting its continuing profile.
    pub fn vacate(&mut self, lane: CanonicalUuid, occupant: CanonicalUuid) {
        self.occupants.remove(&(lane, occupant));
    }

    /// Exact continuing-session key declared by this occupant.
    #[must_use]
    pub fn session(&self, lane: CanonicalUuid, occupant: CanonicalUuid) -> Option<&SessionKey> {
        self.occupants.get(&(lane, occupant))
    }

    /// Profile named by a stable continuing-session key.
    #[must_use]
    pub fn profile(&self, key: &SessionKey) -> Option<&SessionProfile> {
        self.profiles.get(key)
    }

    /// Mutable profile named by a stable continuing-session key.
    #[must_use]
    pub fn profile_mut(&mut self, key: &SessionKey) -> Option<&mut SessionProfile> {
        self.profiles.get_mut(key)
    }

    /// Profiles in deterministic session-key order.
    pub fn iter(&self) -> impl Iterator<Item = (&SessionKey, &SessionProfile)> {
        self.profiles.iter()
    }

    /// Mutable profiles in deterministic session-key order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&SessionKey, &mut SessionProfile)> {
        self.profiles.iter_mut()
    }

    /// The conversation every occupied session holds, counted as the
    /// matchers' background: bounded by the seated occupants, never by
    /// retained history.
    #[must_use]
    pub fn conversation_background(&self) -> Background {
        let occupied = self.occupants.values().collect::<BTreeSet<_>>();
        Background::of(
            occupied
                .into_iter()
                .filter_map(|session| self.profiles.get(session))
                .flat_map(|profile| profile.evidence.conversation_messages()),
        )
    }

    /// Retain one card only when the emitting occupant declared an identity.
    pub fn insert(
        &mut self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        evidence: Evidence<EvidenceKey>,
    ) -> Inserted {
        let Some(key) = self.occupants.get(&(lane, occupant)) else {
            return Inserted::Undeclared;
        };
        let evidence_key = evidence.key().clone();
        let digest = evidence.digest();
        let profile = self
            .profiles
            .get_mut(key)
            .expect("every occupant session key owns a profile");
        if profile.evidence.insert(evidence).is_err() {
            return Inserted::Duplicate;
        }
        let key = key.clone();
        self.push_opportunity(
            key,
            ProfileOpportunityKind::Stored {
                key: evidence_key,
                digest,
            },
        );
        Inserted::Stored
    }

    /// Mark a profile as owing historical acquisition from its beginning.
    pub fn begin_warming(&mut self, key: &SessionKey) {
        if let Some(profile) = self.profiles.get_mut(key) {
            profile.warm = WarmState::Warming {
                cursor: WarmCursor::default(),
            };
        }
    }

    /// Retain one card a newest-first history walk found for `key`, older than
    /// every retained card of its class.
    pub fn retain_older(&mut self, key: &SessionKey, evidence: Evidence<EvidenceKey>) -> Inserted {
        let Some(profile) = self.profiles.get_mut(key) else {
            return Inserted::Undeclared;
        };
        let evidence_key = evidence.key().clone();
        let digest = evidence.digest();
        let kind = evidence.kind();
        if profile.evidence.insert_older(evidence).is_err() {
            return if profile.evidence.is_full(kind) {
                Inserted::Full
            } else {
                Inserted::Duplicate
            };
        }
        self.push_opportunity(
            key.clone(),
            ProfileOpportunityKind::Stored {
                key: evidence_key,
                digest,
            },
        );
        Inserted::Stored
    }

    /// The completion cursor the next shared task-history step resumes from,
    /// when any profile still owes completion history.
    #[must_use]
    pub fn owed_facts(&self) -> Option<Option<Seq>> {
        self.profiles
            .values()
            .filter(|profile| profile.owes())
            .map(|profile| profile.warm.cursor())
            .find(|cursor| !cursor.facts_complete)
            .map(|cursor| cursor.facts)
    }

    /// The first profile, in key order, still owing transcript history, and
    /// the oldest position its walk has crossed.
    #[must_use]
    pub fn owed_transcript(&self) -> Option<(&SessionKey, Option<TranscriptCursor>)> {
        self.profiles.iter().find_map(|(key, profile)| {
            let cursor = profile.warm.cursor();
            (profile.owes() && !cursor.transcript_complete).then_some((key, cursor.transcript))
        })
    }

    /// Complete the sources of every owing profile whose class is already full
    /// of newer cards, without reading the history it would otherwise owe.
    pub fn settle_full(&mut self) {
        let mut ready = Vec::new();
        for (key, profile) in &mut self.profiles {
            let cursor = profile.warm.cursor();
            let newly_full = (!cursor.facts_complete && profile.evidence.is_full(Kind::Task))
                || (!cursor.transcript_complete && profile.evidence.is_full(Kind::Conversation));
            if profile.owes()
                && newly_full
                && let Some(through) = profile.settle(cursor)
            {
                ready.push((key.clone(), through));
            }
        }
        for (key, through) in ready {
            self.push_opportunity(key, ProfileOpportunityKind::Ready { through });
        }
    }

    /// Record one bounded successful transcript step for `key`: its walk has
    /// crossed back to `crossed`, which is the beginning when `complete`.
    pub fn warmed_transcript(
        &mut self,
        key: &SessionKey,
        crossed: TranscriptCursor,
        complete: bool,
    ) {
        let Some(profile) = self.profiles.get_mut(key).filter(|profile| profile.owes()) else {
            return;
        };
        let mut cursor = profile.warm.cursor();
        cursor.transcript = Some(crossed);
        cursor.transcript_complete = complete;
        if let Some(through) = profile.settle(cursor) {
            self.push_opportunity(key.clone(), ProfileOpportunityKind::Ready { through });
        }
    }

    /// Apply one shared bounded completion-history step to every profile
    /// still owing completion history; `next` resumes the following step.
    pub fn warmed_facts(&mut self, next: Option<Seq>, complete: bool) {
        let mut ready = Vec::new();
        for (key, profile) in &mut self.profiles {
            let mut cursor = profile.warm.cursor();
            if !profile.owes() || cursor.facts_complete {
                continue;
            }
            cursor.facts = next;
            cursor.facts_complete = complete;
            if let Some(through) = profile.settle(cursor) {
                ready.push((key.clone(), through));
            }
        }
        for (key, through) in ready {
            self.push_opportunity(key, ProfileOpportunityKind::Ready { through });
        }
    }

    /// Fail every profile still owing completion history, keeping its cards.
    pub fn facts_failed(&mut self, detail: impl Into<String>) {
        let detail = detail.into();
        for profile in self.profiles.values_mut() {
            if profile.owes() && !profile.warm.cursor().facts_complete {
                profile.warm = WarmState::Failed {
                    cursor: profile.warm.cursor(),
                    detail: detail.clone(),
                };
            }
        }
    }

    /// Record a failed warm step without discarding cards already acquired.
    pub fn warm_failed(&mut self, key: &SessionKey, detail: impl Into<String>) {
        let Some(profile) = self.profiles.get_mut(key) else {
            return;
        };
        profile.warm = WarmState::Failed {
            cursor: profile.warm.cursor(),
            detail: detail.into(),
        };
    }

    /// Record that one Design statement may change selection for every profile.
    pub fn design_changed(&mut self, statement: Seq) {
        let profiles = &self.profiles;
        let opportunities = &mut self.opportunities;
        let next = &mut self.next_opportunity;
        for session in profiles.keys() {
            push_profile_opportunity(
                opportunities,
                next,
                session.clone(),
                ProfileOpportunityKind::DesignChanged { statement },
            );
        }
    }

    /// Read a bounded page after a process-local cursor.
    #[must_use]
    pub fn opportunities(&self, after: Option<u64>, limit: usize) -> ProfileOpportunityPage {
        let retained_first = self.opportunities.front().map(|row| row.cursor);
        let start = after.unwrap_or(0);
        let missed = retained_first.is_some_and(|first| start.saturating_add(1) < first);
        let mut matching = self.opportunities.iter().filter(|row| row.cursor > start);
        let rows = matching
            .by_ref()
            .take(limit.min(PROFILE_OPPORTUNITIES))
            .cloned()
            .collect::<Vec<_>>();
        let more = matching.next().is_some();
        let next = rows.last().map(|row| row.cursor).or(after);
        ProfileOpportunityPage {
            rows,
            next,
            missed,
            more,
        }
    }

    fn push_opportunity(&mut self, session: SessionKey, kind: ProfileOpportunityKind) {
        push_profile_opportunity(
            &mut self.opportunities,
            &mut self.next_opportunity,
            session,
            kind,
        );
    }
}

fn push_profile_opportunity(
    opportunities: &mut VecDeque<ProfileOpportunity>,
    next: &mut u64,
    session: SessionKey,
    kind: ProfileOpportunityKind,
) {
    *next = next
        .checked_add(1)
        .expect("profile opportunity cursor exhausted");
    if opportunities.len() == PROFILE_OPPORTUNITIES {
        opportunities.pop_front();
    }
    opportunities.push_back(ProfileOpportunity {
        cursor: *next,
        session,
        kind,
    });
}

#[cfg(test)]
mod tests;
