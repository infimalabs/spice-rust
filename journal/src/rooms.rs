//! Room, working-group, and preparation authority folded from immutable facts.
//!
//! The current projection: room publication positions, working-group heads with
//! their ordered anchors, write-once proposal heads, and one authorization
//! record per selected output and decision. One fold derives that projection.
//! A commit runs it against stored authority before any head moves, so an
//! unsound reference refuses the whole write; verification and reindexing run
//! it over the complete history, so a stored row is only ever one the fold
//! derives.
//!
//! Source bytes stay with their owners: a projection row names a fact
//! position, a task, or a canonical link, never copied conversation text.
//! Transcript links name a nonempty span within an accounted generation
//! boundary; a matching stream and source identity alone are not authority.

use std::collections::BTreeMap;

use wire::principal::ActingPrincipal;
use wire::rooms::{
    CandidateCoverage, CandidateCovered, CandidateCritiqued, CandidateWithdrawn, CoverageAuthority,
    DesignProposed, ExecutionAuthorized, PreparationDecided, PreparationDecision, ProposalRef,
    RoomAnchor, RoomAuthorityStamp, RoomContributed, RoomDelivered, RoomHorizon, RoomLeft,
    RoomMessaged, RoomPublicationContent, RoomPublished, RoomSource, TranscriptHunk,
    WorkingGroupMoved, WorkingGroupPartition, WorkingGroupsArranged,
};
use wire::{
    Actor, AnyFact, CanonicalUuid, Instant, Seq, TaskPreparation, TaskStamp, TranscriptSourceHead,
};

mod awareness;
mod conversation;
mod coordination;
mod evidence;
mod fold;
mod preparation;
mod relevance;
mod retained_note;
mod sql;

pub(crate) use awareness::require_deliveries as require_return_deliveries;
pub(crate) use conversation::{
    ITEM_SCHEMA_SQL as CONVERSATION_ITEM_SCHEMA_SQL, SCHEMA_SQL as CONVERSATION_SCHEMA_SQL,
    require_deliveries as require_conversation_deliveries,
};
pub(crate) use coordination::{
    Direction, Record as CoordinationRecord, SCHEMA_SQL as COORDINATION_SCHEMA_SQL,
    require_writes as require_coordination_writes,
};
pub(crate) use evidence::{SCHEMA_SQL as EVIDENCE_SCHEMA_SQL, fold as fold_evidence};
use fold::{
    acts_as_principal, is_principal, publish, record, require_hunk, require_operator,
    require_proposal, require_source, require_task,
};
pub(crate) use fold::{fold, require_packet_routes, subject};
pub(crate) use relevance::SCHEMA_SQL as RELEVANCE_SCHEMA_SQL;
pub(crate) use retained_note::{AUDIT_PREFIX, presented};
pub(crate) const MATURITY_SCHEMA_SQL: &str = r#"
CREATE TABLE design_notices (
    design TEXT NOT NULL,
    seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    PRIMARY KEY (design, seq)
) STRICT, WITHOUT ROWID;
CREATE INDEX design_notices_design_seq ON design_notices(design, seq);
CREATE TABLE design_maturity (
    design TEXT PRIMARY KEY,
    body BLOB NOT NULL
) STRICT;
"#;
pub(crate) use sql::{Stored, require_open_controllers, settle_controls, write_projection};

/// Room-authority projection tables, created with the current schema.
pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE room_publications (
    room TEXT NOT NULL,
    publication TEXT NOT NULL,
    seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    PRIMARY KEY (room, publication)
) STRICT, WITHOUT ROWID;
CREATE INDEX room_publications_room_seq ON room_publications(room, seq);
CREATE TABLE proposal_heads (
    subject TEXT PRIMARY KEY,
    controller TEXT NOT NULL,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX proposal_heads_controller_seq ON proposal_heads(controller, head_seq);
CREATE TABLE authorization_heads (
    output TEXT NOT NULL,
    decision TEXT NOT NULL,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL,
    PRIMARY KEY (output, decision)
) STRICT, WITHOUT ROWID;
CREATE INDEX authorization_heads_decision ON authorization_heads(decision);
"#;

/// The working-group partition head, replacing per-group heads.
pub(crate) const GROUP_SCHEMA_SQL: &str = r#"
DROP TABLE IF EXISTS group_anchors;
DROP TABLE IF EXISTS group_heads;
CREATE TABLE group_partition (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
"#;

/// Singleton repository OOPS-generation head.
pub(crate) const OOPS_GENERATION_SCHEMA_SQL: &str = r#"
CREATE TABLE oops_generation_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
"#;

/// Per-lane room catch-up cursors, created with the current schema.
pub(crate) const CURSOR_SCHEMA_SQL: &str = r#"
CREATE TABLE room_cursors (
    lane TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
"#;

/// Per-lane room catch-up horizons, created with the current schema.
pub(crate) const HORIZON_SCHEMA_SQL: &str = r#"
CREATE TABLE room_horizons (
    lane TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
"#;

/// Why the fold could not admit one fact.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The fact names absent, stale, or contradictory authority.
    Unsound(String),
    /// Stored authority could not be read or written.
    Store(rusqlite::Error),
    /// A stored or proposed value did not cross the canonical codec.
    Codec(serde_json::Error),
}

impl From<rusqlite::Error> for Refusal {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for Refusal {
    fn from(error: serde_json::Error) -> Self {
        Self::Codec(error)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsound(detail) => formatter.write_str(detail),
            Self::Store(error) => write!(formatter, "room authority storage failed: {error}"),
            Self::Codec(error) => write!(formatter, "room authority codec failed: {error}"),
        }
    }
}

fn unsound<T>(detail: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal::Unsound(detail.into()))
}

/// One stored proposal head: its controller, position, and canonical body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProposalRow {
    pub(crate) controller: String,
    pub(crate) seq: u64,
    pub(crate) body: Vec<u8>,
}

/// One stored authorization record for one selected output and decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthorizationRow {
    pub(crate) decision: String,
    pub(crate) seq: u64,
    pub(crate) body: Vec<u8>,
}

/// The complete room-authority projection, row for row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Projection {
    /// `(room, publication)` to publishing fact position.
    pub(crate) publications: BTreeMap<(String, String), u64>,
    /// `(Design, notice position)` rows for bounded per-Design reads.
    pub(crate) notices: BTreeMap<(String, u64), ()>,
    /// Design to its canonical bounded maturity accumulator.
    pub(crate) maturity: BTreeMap<String, Vec<u8>>,
    /// Repository OOPS generation's head position and canonical body.
    pub(crate) oops_generation: Option<(u64, Vec<u8>)>,
    /// The working-group partition's head position and canonical body.
    pub(crate) partition: Option<(u64, Vec<u8>)>,
    /// Canonical proposal subject to its write-once head.
    pub(crate) proposals: BTreeMap<String, ProposalRow>,
    /// `(selected output, decision)` to its authorization record.
    pub(crate) authorizations: BTreeMap<(String, String), AuthorizationRow>,
    /// Logical lane to its catch-up cursor head position and canonical body.
    pub(crate) cursors: BTreeMap<String, (u64, Vec<u8>)>,
    /// Logical lane to its catch-up horizon position and canonical body.
    pub(crate) horizons: BTreeMap<String, (u64, Vec<u8>)>,
    /// Room to its latest `room::directed` head position and canonical body.
    pub(crate) directed: BTreeMap<String, (u64, Vec<u8>)>,
    /// Room to its latest `room::shelved` head position and canonical body.
    pub(crate) shelved: BTreeMap<String, (u64, Vec<u8>)>,
    /// Conversation publications with their operator item and result heads.
    pub(crate) conversation: conversation::Heads,
    /// `(task, result)` required evidence to the position that required it.
    pub(crate) evidence: BTreeMap<(String, String), u64>,
    /// Append-only shadow relevance observations and operator assessments.
    pub(crate) relevance: relevance::Heads,
}

/// Authority one fold step reads and changes.
///
/// Stored authority answers from SQLite inside the commit transaction; history
/// answers from what the fold has already seen. Task preparation is part of
/// this view because output control must name tasks the fold can prove exist
/// and are not already controlled.
pub(crate) trait Authority {
    /// Whether the task exists, and its current preparation when it does.
    fn task(&self, task: &TaskStamp) -> Result<Option<Option<TaskPreparation>>, Refusal>;
    /// Record a task birth or an admitted output control.
    fn set_task(&mut self, task: &TaskStamp, preparation: Option<TaskPreparation>);
    /// Declared intent of an existing task.
    fn task_intent(&self, task: &TaskStamp) -> Result<Option<Option<wire::TaskIntent>>, Refusal>;
    /// Record an admitted task's declared intent.
    fn set_task_intent(&mut self, task: &TaskStamp, intent: Option<wire::TaskIntent>);
    /// Current ordinary dependency edges of an existing task.
    fn task_dependencies(&self, task: &TaskStamp) -> Result<Option<Vec<TaskStamp>>, Refusal>;
    /// Replace one task's replay-visible ordinary dependencies.
    fn set_task_dependencies(&mut self, task: &TaskStamp, dependencies: Vec<TaskStamp>);
    /// Record dependency authority from a task birth. Replay may restore a
    /// predecessor preparation gate that schema 77 crossed directly into the
    /// statement head; live commits retain the fact's exact dependency list.
    fn set_task_birth_dependencies(&mut self, added: &wire::body::TaskAdded) {
        self.set_task_dependencies(&added.task, added.depends.clone());
    }
    /// Current statement head position of one task, when it exists.
    fn task_statement(&self, task: &TaskStamp) -> Result<Option<Seq>, Refusal>;
    /// Record one task statement mutation.
    fn set_task_statement(&mut self, task: &TaskStamp, statement: Seq);
    /// Current active claim-head position, or none when the task is unclaimed.
    fn active_claim(&self, task: &TaskStamp) -> Result<Option<Seq>, Refusal>;
    /// Replace the replay-visible active claim head.
    fn set_active_claim(&mut self, task: &TaskStamp, claim: Option<Seq>);
    /// Record the current accounted head of one logical transcript stream.
    fn account(&mut self, head: &TranscriptSourceHead);
    /// Current accounted head of the linked logical transcript stream.
    fn transcript(
        &self,
        head: &TranscriptSourceHead,
    ) -> Result<Option<TranscriptSourceHead>, Refusal>;
    /// Whether this room publication already exists.
    fn published(&self, room: &str, publication: &str) -> Result<bool, Refusal>;
    /// Whether this position is an indexed room publication.
    fn publication_at(&self, seq: u64) -> Result<bool, Refusal>;
    /// Whether this position is an indexed Design request.
    fn request_at(&self, seq: u64) -> Result<bool, Refusal>;
    /// The canonical body of one lane's current catch-up cursor.
    fn cursor(&self, lane: &str) -> Result<Option<Vec<u8>>, Refusal>;
    /// Whether one lane already has a catch-up horizon.
    fn horizon(&self, lane: &str) -> Result<bool, Refusal>;
    /// The canonical body of the current working-group partition.
    fn group_partition(&self) -> Result<Option<Vec<u8>>, Refusal>;
    /// Canonical body of the current repository OOPS generation.
    fn oops_generation(&self) -> Result<Option<Vec<u8>>, Refusal>;
    /// The canonical body of one proposal head.
    fn proposal(&self, subject: &str) -> Result<Option<Vec<u8>>, Refusal>;
    /// The actor that recorded one proposal head.
    fn proposal_author(&self, subject: &str) -> Result<Option<Actor>, Refusal>;
    /// Whether this exact candidate already has an operator decision.
    fn candidate_decided(&self, candidate: &wire::rooms::CandidateRef) -> Result<bool, Refusal>;
    /// Whether this decision already authorized a selection.
    fn decision_authorized(&self, decision: &str) -> Result<bool, Refusal>;
    /// The room-coordination direction one steering key names, if any.
    fn direction(&self, key: &str) -> Result<Option<Direction>, Refusal>;
    /// Record a room-coordination direction's current state.
    fn set_direction(&mut self, key: &str, direction: Direction);
    /// The lane the operator addressed with direction `key`, if the operator
    /// sent one under that key.
    fn operator_direction(&self, key: &str) -> Result<Option<CanonicalUuid>, Refusal>;
    /// Record the lane one operator direction addresses.
    fn set_operator_direction(&mut self, key: &str, lane: CanonicalUuid);
    /// One room's latest coordination record of a kind: position and body.
    fn coordination(
        &self,
        record: CoordinationRecord,
        room: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, Refusal>;
    /// The canonical body of one admitted conversation publication.
    fn conversation(&self, publication: &str) -> Result<Option<Vec<u8>>, Refusal>;
    /// One operator item head by its key.
    fn room_item(&self, item: &str) -> Result<Option<conversation::ItemRow>, Refusal>;
    /// One question result head by its handle.
    fn question_result(&self, result: &str) -> Result<Option<conversation::ResultRow>, Refusal>;
    /// Results one task currently requires answered, in canonical order.
    fn evidence(&self, task: &TaskStamp) -> Result<Vec<wire::rooms::QuestionResultRef>, Refusal>;
    /// How many question results one requester holds pending.
    fn pending_results(&self, requester: &str) -> Result<usize, Refusal>;
    /// One shadow relevance observation by stable identity.
    fn relevance_observation(
        &self,
        observation: &str,
    ) -> Result<Option<relevance::ObservationRow>, Refusal>;
    /// One operator assessment by observation identity.
    fn relevance_assessment(
        &self,
        observation: &str,
    ) -> Result<Option<relevance::AssessmentRow>, Refusal>;
    /// Apply one projection change.
    fn change(&mut self, change: Change) -> Result<(), Refusal>;
}

/// One projection consequence of an admitted fact.
pub(crate) enum Change {
    Publication {
        room: String,
        publication: String,
        seq: u64,
    },
    GroupPartition {
        seq: u64,
        body: Vec<u8>,
    },
    OopsGeneration {
        seq: u64,
        body: Vec<u8>,
    },
    Proposal {
        subject: String,
        row: ProposalRow,
    },
    Authorization {
        outputs: Vec<String>,
        row: AuthorizationRow,
    },
    Cursor {
        lane: String,
        seq: u64,
        body: Vec<u8>,
    },
    Horizon {
        lane: String,
        seq: u64,
        body: Vec<u8>,
    },
    Coordination {
        record: CoordinationRecord,
        room: String,
        seq: u64,
        body: Vec<u8>,
    },
    Conversation(conversation::Edit),
    Evidence {
        task: String,
        released: Option<String>,
        required: Option<(String, u64)>,
    },
    Relevance(relevance::Edit),
    MaturityNotice {
        seq: u64,
        notice: wire::rooms::DesignNoticed,
    },
    MaturityContribution(wire::rooms::RoomContributed),
    MaturityLeave(RoomLeft),
}

impl Projection {
    fn apply(&mut self, change: Change) -> Result<(), Refusal> {
        match change {
            Change::Publication {
                room,
                publication,
                seq,
            } => {
                self.publications.insert((room, publication), seq);
            }
            Change::GroupPartition { seq, body } => {
                self.partition = Some((seq, body));
            }
            Change::OopsGeneration { seq, body } => {
                self.oops_generation = Some((seq, body));
            }
            Change::Proposal { subject, row } => {
                self.proposals.insert(subject, row);
            }
            Change::Authorization { outputs, row } => {
                for output in outputs {
                    self.authorizations
                        .insert((output, row.decision.clone()), row.clone());
                }
            }
            Change::Cursor { lane, seq, body } => {
                self.cursors.insert(lane, (seq, body));
            }
            Change::Horizon { lane, seq, body } => {
                self.horizons.insert(lane, (seq, body));
            }
            Change::Coordination {
                record,
                room,
                seq,
                body,
            } => {
                let heads = match record {
                    CoordinationRecord::Directed => &mut self.directed,
                    CoordinationRecord::Shelved => &mut self.shelved,
                };
                heads.insert(room, (seq, body));
            }
            Change::Conversation(edit) => self.conversation.apply(edit),
            Change::Evidence {
                task,
                released,
                required,
            } => {
                if let Some(result) = released {
                    self.evidence.remove(&(task.clone(), result));
                }
                if let Some((result, seq)) = required {
                    self.evidence.insert((task, result), seq);
                }
            }
            Change::Relevance(edit) => self.relevance.apply(edit),
            Change::MaturityNotice { seq, notice } => {
                self.notices.insert((notice.design.to_string(), seq), ());
                self.update_maturity(&notice.design, |maturity| {
                    maturity.observe_request(&notice);
                })?;
            }
            Change::MaturityContribution(contribution) => {
                self.update_maturity(&contribution.room, |maturity| {
                    maturity.observe_contribution(&contribution);
                })?;
            }
            Change::MaturityLeave(leave) => {
                if self.maturity.contains_key(leave.room.as_str()) {
                    self.update_maturity(&leave.room, |maturity| maturity.observe_leave(&leave))?;
                }
            }
        }
        Ok(())
    }

    fn update_maturity(
        &mut self,
        design: &TaskStamp,
        update: impl FnOnce(&mut wire::rooms::DesignMaturity),
    ) -> Result<(), Refusal> {
        let mut maturity = self
            .maturity
            .get(design.as_str())
            .map(|body| wire::decode(body))
            .transpose()?
            .unwrap_or_default();
        update(&mut maturity);
        self.maturity
            .insert(design.to_string(), wire::encode(&maturity)?);
        Ok(())
    }
}

/// Every task, transcript generation, proposal author, and room-coordination
/// direction the history has introduced, beside the projection it derives.
#[derive(Default)]
pub(crate) struct History {
    projection: Projection,
    tasks: BTreeMap<String, Option<TaskPreparation>>,
    task_intents: BTreeMap<String, Option<wire::TaskIntent>>,
    task_dependencies: BTreeMap<String, Vec<TaskStamp>>,
    task_statements: BTreeMap<String, Seq>,
    active_claims: BTreeMap<String, Seq>,
    transcripts: BTreeMap<String, TranscriptSourceHead>,
    /// Proposal head position to the actor that recorded it.
    authors: BTreeMap<u64, Actor>,
    /// Steering key to the room-coordination direction it names.
    directions: BTreeMap<String, Direction>,
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
            // A proposal's author decides whether later coverage is
            // independent of it.
            if fact.read::<DesignProposed>().is_some() {
                history.authors.insert(seq.get(), actor.clone());
            }
            if let Err(refusal) = evidence::fold(&mut history, seq, actor, fact) {
                refusals.push((seq, refusal));
                continue;
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
        Ok(self.task_dependencies.get(task.as_str()).cloned())
    }

    fn set_task_dependencies(&mut self, task: &TaskStamp, dependencies: Vec<TaskStamp>) {
        self.task_dependencies
            .insert(task.to_string(), dependencies);
    }
    fn set_task_birth_dependencies(&mut self, added: &wire::body::TaskAdded) {
        let mut dependencies = added.depends.clone();
        if let Some(preparation) = &added.preparation
            && preparation.construction == added.task
            && !dependencies.contains(&preparation.parent)
        {
            dependencies.push(preparation.parent.clone());
        }
        self.set_task_dependencies(&added.task, dependencies);
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

    fn direction(&self, key: &str) -> Result<Option<Direction>, Refusal> {
        Ok(self.directions.get(key).cloned())
    }

    fn set_direction(&mut self, key: &str, direction: Direction) {
        self.directions.insert(key.to_owned(), direction);
    }

    fn operator_direction(&self, key: &str) -> Result<Option<CanonicalUuid>, Refusal> {
        Ok(self.operator_directions.get(key).copied())
    }

    fn set_operator_direction(&mut self, key: &str, lane: CanonicalUuid) {
        self.operator_directions.insert(key.to_owned(), lane);
    }

    fn coordination(
        &self,
        record: CoordinationRecord,
        room: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, Refusal> {
        let heads = match record {
            CoordinationRecord::Directed => &self.projection.directed,
            CoordinationRecord::Shelved => &self.projection.shelved,
        };
        Ok(heads.get(room).cloned())
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

    fn question_result(&self, result: &str) -> Result<Option<conversation::ResultRow>, Refusal> {
        Ok(self.projection.conversation.results.get(result).cloned())
    }

    fn evidence(&self, task: &TaskStamp) -> Result<Vec<wire::rooms::QuestionResultRef>, Refusal> {
        evidence::parse(
            self.projection
                .evidence
                .keys()
                .filter(|(owner, _)| owner == task.as_str())
                .map(|(_, result)| result.clone()),
        )
    }

    fn pending_results(&self, requester: &str) -> Result<usize, Refusal> {
        Ok(self.projection.conversation.pending_results(requester))
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
