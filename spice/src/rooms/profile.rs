//! Evidence ownership for continuing vendor sessions.
//!
//! A logical lane can host many unrelated conversations, and one conversation
//! can resume under several occupants or tape generations. [`SessionKey`]
//! therefore uses only the lane, vendor, and vendor-declared resumable identity.
//! Occupants are lookup witnesses; they are never profile identities.

use std::collections::{BTreeMap, VecDeque};

use wire::{CanonicalUuid, Name, Seq, TaskStamp, body::LaneSessionDeclared};

use super::evidence::{Digest, Evidence, EvidenceProfile};

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
    /// Exact statement revision and successful completion observed together.
    Task {
        task: TaskStamp,
        statement: Seq,
        completion: Seq,
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
    /// Last journal fact already folded.
    pub facts: Option<Seq>,
    /// Oldest transcript position already crossed.
    pub transcript: Option<TranscriptCursor>,
    /// Whether the retained task-completion history reached its beginning.
    pub facts_complete: bool,
    /// Whether the retained transcript history reached its beginning.
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
}

/// Result of attempting to retain one card for an occupant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Inserted {
    /// The occupant has no admitted vendor-session declaration.
    Undeclared,
    /// An exact retained content digest already represented this evidence.
    Duplicate,
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

    /// Record one bounded successful warm step.
    pub fn warmed(&mut self, key: &SessionKey, through: WarmCursor, complete: bool) {
        let Some(profile) = self.profiles.get_mut(key) else {
            return;
        };
        let was_ready = profile.warm.is_ready();
        let mut through = through;
        through.transcript_complete = complete;
        profile.warm = if through.facts_complete && through.transcript_complete {
            WarmState::Ready { through }
        } else {
            WarmState::Incomplete { cursor: through }
        };
        if !was_ready && profile.warm.is_ready() {
            self.push_opportunity(key.clone(), ProfileOpportunityKind::Ready { through });
        }
    }

    /// Apply one shared bounded task-history step to every seated profile.
    pub fn warmed_facts(&mut self, through: Option<Seq>, complete: bool) {
        let profiles = &mut self.profiles;
        let opportunities = &mut self.opportunities;
        let next = &mut self.next_opportunity;
        for (key, profile) in profiles {
            if matches!(profile.warm, WarmState::Failed { .. }) {
                continue;
            }
            let was_ready = profile.warm.is_ready();
            let mut cursor = profile.warm.cursor();
            cursor.facts = through;
            cursor.facts_complete = complete;
            profile.warm = if cursor.facts_complete && cursor.transcript_complete {
                WarmState::Ready { through: cursor }
            } else {
                WarmState::Incomplete { cursor }
            };
            if !was_ready && profile.warm.is_ready() {
                push_profile_opportunity(
                    opportunities,
                    next,
                    key.clone(),
                    ProfileOpportunityKind::Ready { through: cursor },
                );
            }
        }
    }

    /// Mark every currently known profile failed without discarding evidence.
    pub fn warm_all_failed(&mut self, detail: impl Into<String>) {
        let detail = detail.into();
        for profile in self.profiles.values_mut() {
            profile.warm = WarmState::Failed {
                cursor: profile.warm.cursor(),
                detail: detail.clone(),
            };
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
