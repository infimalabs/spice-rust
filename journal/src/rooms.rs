//! Room, working-group, and preparation authority folded from immutable facts.
//!
//! Facts own every room-authority value; the tables declared here are its
//! current projection: room publication positions, working-group heads with
//! their ordered anchors, write-once proposal heads, and one authorization head
//! per selected output. One fold derives that projection. A commit runs it
//! against stored authority before any head moves, so an unsound reference
//! refuses the whole write; verification and reindexing run it over the
//! complete history, so a stored row is only ever one the fold derives.
//!
//! Source bytes stay with their owners: a projection row names a fact
//! position, a task, or a canonical link, never copied conversation text.
//! Transcript links name a nonempty span within an accounted generation
//! boundary; a matching stream and source identity alone are not authority.

use std::collections::{BTreeMap, BTreeSet};

use wire::rooms::{
    CandidateCoverage, CandidateCovered, CandidateCritiqued, CandidateRetained, CoverageAuthority,
    DecisionOutcome, ExecutionAuthorized, PreparationDecided, PreparationDecision, ProposalRef,
    RetainedCandidate, RoomAuthorityStamp, RoomCoordinationStarted, RoomDelivered, RoomMessaged,
    RoomPublicationContent, RoomPublished, RoomPutAside, RoomSource, TranscriptHunk,
    WorkingGroupRemoved, WorkingGroupStored,
};
use wire::{Actor, AnyFact, Seq, TaskPreparation, TaskStamp, TranscriptSourceHead};

mod sql;

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
CREATE TABLE group_heads (
    working_group TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE group_anchors (
    anchor TEXT NOT NULL,
    working_group TEXT NOT NULL REFERENCES group_heads(working_group),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (anchor, working_group),
    UNIQUE (working_group, ordinal)
) STRICT, WITHOUT ROWID;
CREATE TABLE proposal_heads (
    subject TEXT PRIMARY KEY,
    controller TEXT NOT NULL,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX proposal_heads_controller_seq ON proposal_heads(controller, head_seq);
CREATE TABLE authorization_heads (
    output TEXT PRIMARY KEY,
    decision TEXT NOT NULL,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX authorization_heads_decision ON authorization_heads(decision);
"#;

/// Per-lane room catch-up cursors, created with the current schema.
pub(crate) const CURSOR_SCHEMA_SQL: &str = r#"
CREATE TABLE room_cursors (
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

/// One stored authorization head for a selected output.
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
    /// Working group to its head position and canonical body.
    pub(crate) groups: BTreeMap<String, (u64, Vec<u8>)>,
    /// `(anchor, group)` to the anchor's position in that group's order.
    pub(crate) anchors: BTreeMap<(String, String), i64>,
    /// Canonical proposal subject to its write-once head.
    pub(crate) proposals: BTreeMap<String, ProposalRow>,
    /// Selected output to its authorization head.
    pub(crate) authorizations: BTreeMap<String, AuthorizationRow>,
    /// Logical lane to its catch-up cursor head position and canonical body.
    pub(crate) cursors: BTreeMap<String, (u64, Vec<u8>)>,
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
    /// The canonical body of one lane's current catch-up cursor.
    fn cursor(&self, lane: &str) -> Result<Option<Vec<u8>>, Refusal>;
    /// Whether this working group currently exists.
    fn grouped(&self, group: &str) -> Result<bool, Refusal>;
    /// The canonical body of one proposal head.
    fn proposal(&self, subject: &str) -> Result<Option<Vec<u8>>, Refusal>;
    /// The actor that recorded one proposal head.
    fn proposal_author(&self, subject: &str) -> Result<Option<Actor>, Refusal>;
    /// Whether this output already carries an authorization.
    fn output_authorized(&self, output: &str) -> Result<bool, Refusal>;
    /// Whether this decision already authorized a selection.
    fn decision_authorized(&self, decision: &str) -> Result<bool, Refusal>;
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
    GroupStored {
        group: String,
        seq: u64,
        body: Vec<u8>,
        anchors: Vec<String>,
    },
    GroupRemoved {
        group: String,
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
}

impl Projection {
    fn apply(&mut self, change: Change) {
        match change {
            Change::Publication {
                room,
                publication,
                seq,
            } => {
                self.publications.insert((room, publication), seq);
            }
            Change::GroupStored {
                group,
                seq,
                body,
                anchors,
            } => {
                self.anchors.retain(|(_, owner), _| owner != &group);
                for (ordinal, anchor) in (0_i64..).zip(anchors) {
                    self.anchors.insert((anchor, group.clone()), ordinal);
                }
                self.groups.insert(group, (seq, body));
            }
            Change::GroupRemoved { group } => {
                self.anchors.retain(|(_, owner), _| owner != &group);
                self.groups.remove(&group);
            }
            Change::Proposal { subject, row } => {
                self.proposals.insert(subject, row);
            }
            Change::Authorization { outputs, row } => {
                for output in outputs {
                    self.authorizations.insert(output, row.clone());
                }
            }
            Change::Cursor { lane, seq, body } => {
                self.cursors.insert(lane, (seq, body));
            }
        }
    }
}

/// Every task, transcript generation, and proposal author the history has
/// introduced, beside the projection the history derives.
#[derive(Default)]
pub(crate) struct History {
    projection: Projection,
    tasks: BTreeMap<String, Option<TaskPreparation>>,
    transcripts: BTreeMap<String, TranscriptSourceHead>,
    /// Proposal head position to the actor that recorded it.
    authors: BTreeMap<u64, Actor>,
}

impl History {
    /// Fold the complete canonical history, collecting every refusal by
    /// position instead of stopping at the first.
    pub(crate) fn rebuild<'a>(
        facts: impl IntoIterator<Item = (Seq, &'a Actor, &'a AnyFact)>,
    ) -> (Projection, Vec<(Seq, Refusal)>) {
        let mut history = Self::default();
        let mut refusals = Vec::new();
        for (seq, actor, fact) in facts {
            // A retained candidate's author decides whether later coverage is
            // independent of it.
            if fact.read::<CandidateRetained>().is_some() {
                history.authors.insert(seq.get(), actor.clone());
            }
            if let Err(refusal) = fold(&mut history, seq, actor, fact) {
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

    fn cursor(&self, lane: &str) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .projection
            .cursors
            .get(lane)
            .map(|(_, body)| body.clone()))
    }

    fn grouped(&self, group: &str) -> Result<bool, Refusal> {
        Ok(self.projection.groups.contains_key(group))
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

    fn output_authorized(&self, output: &str) -> Result<bool, Refusal> {
        Ok(self.projection.authorizations.contains_key(output))
    }

    fn decision_authorized(&self, decision: &str) -> Result<bool, Refusal> {
        Ok(self
            .projection
            .authorizations
            .values()
            .any(|row| row.decision == decision))
    }

    fn change(&mut self, change: Change) -> Result<(), Refusal> {
        self.projection.apply(change);
        Ok(())
    }
}

/// Canonical key of one proposal subject.
pub(crate) fn subject(proposal: &ProposalRef) -> Result<String, Refusal> {
    String::from_utf8(wire::encode(proposal)?)
        .map_err(|_| Refusal::Unsound("proposal subject is not text".to_owned()))
}

/// Admit one fact `actor` wrote against `authority` and apply its projection
/// consequence.
pub(crate) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    fact: &AnyFact,
) -> Result<(), Refusal> {
    if let Some(body) = fact.read::<wire::rooms::OutputControlled>() {
        return control(authority, &body.output, body.preparation);
    }
    if let Some((task, preparation)) = fact.task_preparation() {
        authority.set_task(task, preparation.cloned());
        return Ok(());
    }
    if let Some((task, _)) = fact.declared_intent() {
        // Creation returned above, so this is an edit changing declared intent.
        // Intent is what separates an associated preparation from a gated
        // output, so an associated task keeps the intent it was created with.
        if let Some(Some(preparation)) = authority.task(task)? {
            return unsound(format!(
                "task {task} under controller {} keeps the intent it was created with",
                preparation.controller
            ));
        }
        return Ok(());
    }
    if let Some(body) = fact.read::<wire::body::JournalIngested>() {
        authority.account(&TranscriptSourceHead {
            lane: body.lane,
            vendor: body.vendor,
            stream: body.stream,
            source: body.source,
            next: body.to,
        });
        return Ok(());
    }
    if let Some(body) = fact.read::<RoomPublished>() {
        if let RoomPublicationContent::Linked { link } = &body.content {
            require_source(authority, seq, link)?;
        }
        return publish(authority, seq, &body.room, &body.publication);
    }
    if let Some(body) = fact.read::<RoomMessaged>() {
        if *actor != Actor::Lane(body.hunk.head.lane) {
            return unsound(format!(
                "room packet in {} is written by {actor}, not its hunk's lane {}",
                body.room, body.hunk.head.lane
            ));
        }
        require_hunk(authority, &body.hunk)?;
        return publish(authority, seq, &body.room, &body.publication);
    }
    if let Some(result) = fold_coordination(authority, seq, fact) {
        return result;
    }
    if let Some(WorkingGroupStored { group }) = fact.read() {
        for anchor in &group.anchors {
            require_task(authority, anchor, "working-group anchor")?;
        }
        let key = group.group.to_string();
        return authority.change(Change::GroupStored {
            group: key,
            seq: seq.get(),
            body: wire::encode(&group)?,
            anchors: group.anchors.iter().map(ToString::to_string).collect(),
        });
    }
    if let Some(WorkingGroupRemoved { group }) = fact.read() {
        let key = group.to_string();
        if !authority.grouped(&key)? {
            return unsound(format!("working group {key} does not exist"));
        }
        return authority.change(Change::GroupRemoved { group: key });
    }
    if let Some(CandidateRetained { candidate }) = fact.read() {
        return retain(authority, seq, candidate);
    }
    if let Some(CandidateCritiqued { critique }) = fact.read() {
        let assessed = ProposalRef::Candidate {
            candidate: critique.critique.candidate.clone(),
        };
        require_proposal(authority, &assessed)?;
        require_source(authority, seq, &critique.source)?;
        let controller = critique.critique.candidate.controller.to_string();
        let proposal = ProposalRef::Critique {
            critique: critique.critique.clone(),
        };
        return record(authority, seq, &proposal, controller, &critique);
    }
    if let Some(CandidateCovered { coverage }) = fact.read() {
        return cover(authority, seq, actor, coverage);
    }
    if let Some(PreparationDecided { decision }) = fact.read() {
        require_operator(actor, "a preparation decision")?;
        return decide(authority, seq, decision);
    }
    if let Some(ExecutionAuthorized { authorization }) = fact.read() {
        require_operator(actor, "execution authorization")?;
        return authorize(authority, seq, authorization);
    }
    Ok(())
}

fn fold_coordination(
    authority: &mut impl Authority,
    seq: Seq,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    if let Some(body) = fact.read::<RoomCoordinationStarted>() {
        return Some(publish(authority, seq, &body.room, &body.publication));
    }
    if let Some(body) = fact.read::<RoomPutAside>() {
        return Some(publish(authority, seq, &body.room, &body.publication));
    }
    fact.read::<RoomDelivered>()
        .map(|body| deliver(authority, seq, &body))
}

/// Each room record of a packet names exactly the directives the same write's
/// peer messages opened into that room, in write order, and each of those
/// messages was sent from the packet's lane by its author at its hunk's tape
/// record. A write publishes at most one packet, so these are its routes.
pub(crate) fn require_packet_routes<'a>(
    facts: impl IntoIterator<Item = &'a AnyFact>,
) -> Result<(), Refusal> {
    let mut messages = Vec::new();
    let mut packets = Vec::new();
    for fact in facts {
        if let Some(message) = fact.read::<wire::body::SteerMessaged>() {
            messages.push(message);
        } else if let Some(packet) = fact.read::<RoomMessaged>() {
            packets.push(packet);
        }
    }
    for packet in &packets {
        let opened = messages
            .iter()
            .filter(|message| message.room.as_ref() == Some(&packet.room))
            .collect::<Vec<_>>();
        if !opened
            .iter()
            .map(|message| message.directive)
            .eq(packet.directed.iter().copied())
        {
            return unsound(format!(
                "room packet in {} names directed routes other than those its write opened there",
                packet.room
            ));
        }
        let hunk = &packet.hunk;
        if let Some(message) = opened.into_iter().find(|message| {
            let locus = &message.locus;
            message.from != hunk.head.lane
                || message.sender != packet.author
                || locus.source != Some(hunk.head.source)
                || (locus.member, locus.offset, locus.length, locus.event)
                    != (
                        hunk.locus.member,
                        hunk.locus.offset,
                        hunk.locus.length,
                        hunk.locus.event,
                    )
        }) {
            return unsound(format!(
                "directive {} in room {} was not sent from this packet's lane by its author at its record",
                message.directive, packet.room
            ));
        }
    }
    Ok(())
}

fn require_operator(actor: &Actor, what: &str) -> Result<(), Refusal> {
    if *actor == Actor::Operator {
        return Ok(());
    }
    unsound(format!("{what} is operator-owned, not written by {actor}"))
}

/// A delivery lands in its own lane's accounted transcript and strictly
/// advances that lane's cursor to an earlier indexed room publication.
fn deliver(
    authority: &mut impl Authority,
    seq: Seq,
    delivered: &RoomDelivered,
) -> Result<(), Refusal> {
    require_hunk(authority, &delivered.hunk)?;
    let through = delivered.through;
    if through.get() >= seq.get() || !authority.publication_at(through.get())? {
        return unsound(format!(
            "delivery cursor {through} names no room publication earlier than {seq}"
        ));
    }
    let lane = delivered.hunk.head.lane.to_string();
    if let Some(previous) = authority.cursor(&lane)? {
        let previous = wire::decode::<RoomDelivered>(&previous)?.through;
        if through <= previous {
            return unsound(format!(
                "lane {lane} delivery cursor {through} does not advance past {previous}"
            ));
        }
    }
    authority.change(Change::Cursor {
        lane,
        seq: seq.get(),
        body: wire::encode(delivered)?,
    })
}

/// Coverage authority is the principal that wrote it: the operator, an
/// independent lane, or the candidate's own authoring lane labeled as
/// self-review.
fn cover(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    coverage: CandidateCoverage,
) -> Result<(), Refusal> {
    let assessed = ProposalRef::Candidate {
        candidate: coverage.coverage.candidate.clone(),
    };
    require_proposal(authority, &assessed)?;
    let author = authority.proposal_author(&subject(&assessed)?)?;
    let admitted = match &coverage.authority {
        CoverageAuthority::Operator {} => *actor == Actor::Operator,
        CoverageAuthority::Independent { lane, .. } => {
            *actor == Actor::Lane(*lane) && author.as_ref() != Some(actor)
        }
        CoverageAuthority::SelfReview { lane, .. } => {
            *actor == Actor::Lane(*lane) && author.as_ref() == Some(actor)
        }
    };
    if !admitted {
        return unsound(format!(
            "coverage authority disagrees with its writer {actor} and the candidate's author"
        ));
    }
    require_source(authority, seq, &coverage.source)?;
    let controller = coverage.coverage.candidate.controller.to_string();
    let proposal = ProposalRef::Coverage {
        coverage: coverage.coverage.clone(),
    };
    record(authority, seq, &proposal, controller, &coverage)
}

fn control(
    authority: &mut impl Authority,
    output: &TaskStamp,
    preparation: TaskPreparation,
) -> Result<(), Refusal> {
    match authority.task(output)? {
        None => return unsound(format!("controlled output {output} does not exist")),
        Some(Some(current)) => {
            return unsound(format!(
                "output {output} is already controlled by {}",
                current.controller
            ));
        }
        Some(None) => {}
    }
    if preparation.parent == *output || preparation.controller == *output {
        return unsound(format!("output {output} cannot prepare or control itself"));
    }
    for task in [
        &preparation.parent,
        &preparation.controller,
        &preparation.construction,
    ] {
        require_task(authority, task, "preparation")?;
    }
    authority.set_task(output, Some(preparation));
    Ok(())
}

fn publish(
    authority: &mut impl Authority,
    seq: Seq,
    room: &TaskStamp,
    publication: &RoomAuthorityStamp,
) -> Result<(), Refusal> {
    require_task(authority, room, "room anchor")?;
    let room = room.to_string();
    let publication = publication.to_string();
    if authority.published(&room, &publication)? {
        return unsound(format!(
            "room {room} publication {publication} already exists"
        ));
    }
    authority.change(Change::Publication {
        room,
        publication,
        seq: seq.get(),
    })
}

fn retain(
    authority: &mut impl Authority,
    seq: Seq,
    candidate: RetainedCandidate,
) -> Result<(), Refusal> {
    let controller = &candidate.candidate.controller;
    require_task(authority, controller, "candidate controller")?;
    for input in &candidate.governed_inputs {
        require_task(authority, input, "governed input")?;
    }
    for output in &candidate.outputs {
        match authority.task(output)? {
            Some(Some(preparation)) if preparation.controller == *controller => {}
            Some(_) => {
                return unsound(format!(
                    "candidate output {output} is not controlled by {controller}"
                ));
            }
            None => return unsound(format!("candidate output {output} does not exist")),
        }
    }
    for reference in &candidate.references {
        require_source(authority, seq, &reference.source)?;
    }
    let proposal = ProposalRef::Candidate {
        candidate: candidate.candidate.clone(),
    };
    record(
        authority,
        seq,
        &proposal,
        controller.to_string(),
        &candidate,
    )
}

fn decide(
    authority: &mut impl Authority,
    seq: Seq,
    decision: PreparationDecision,
) -> Result<(), Refusal> {
    let controller = &decision.decision.controller;
    require_task(authority, controller, "decision controller")?;
    match &decision.outcome {
        DecisionOutcome::Accepted { candidate } | DecisionOutcome::Returned { candidate } => {
            require_proposal(
                authority,
                &ProposalRef::Candidate {
                    candidate: candidate.clone(),
                },
            )?;
        }
        DecisionOutcome::Negative { .. } => {}
    }
    require_source(authority, seq, &decision.source)?;
    let proposal = ProposalRef::Decision {
        decision: decision.decision.clone(),
    };
    record(authority, seq, &proposal, controller.to_string(), &decision)
}

fn authorize(
    authority: &mut impl Authority,
    seq: Seq,
    authorization: wire::rooms::ExecutionAuthorization,
) -> Result<(), Refusal> {
    let decision_ref = ProposalRef::Decision {
        decision: authorization.decision.clone(),
    };
    let decision: PreparationDecision = wire::decode(&require_proposal(authority, &decision_ref)?)?;
    match &decision.outcome {
        DecisionOutcome::Accepted { candidate } if *candidate == authorization.candidate => {}
        _ => {
            return unsound("authorization requires a decision accepting exactly its candidate");
        }
    }
    let candidate: RetainedCandidate = wire::decode(&require_proposal(
        authority,
        &ProposalRef::Candidate {
            candidate: authorization.candidate.clone(),
        },
    )?)?;
    let inputs = |tasks: &[TaskStamp]| tasks.iter().cloned().collect::<BTreeSet<_>>();
    if inputs(&authorization.governed_inputs) != inputs(&candidate.governed_inputs) {
        return unsound("authorization governed inputs differ from the candidate's");
    }
    if let Some(output) = authorization
        .selected_outputs
        .iter()
        .find(|output| !candidate.outputs.contains(output))
    {
        return unsound(format!(
            "selected output {output} is not an output of the candidate"
        ));
    }
    let mut certified = false;
    for coverage in &authorization.coverage {
        let coverage: CandidateCoverage = wire::decode(&require_proposal(
            authority,
            &ProposalRef::Coverage {
                coverage: coverage.clone(),
            },
        )?)?;
        certified |= !matches!(coverage.authority, CoverageAuthority::SelfReview { .. });
    }
    if !certified {
        return unsound(
            "authorization requires independent or operator coverage; self-review alone certifies nothing",
        );
    }
    let decision_key = subject(&decision_ref)?;
    if authority.decision_authorized(&decision_key)? {
        return unsound("decision already authorized a selection");
    }
    let outputs = authorization
        .selected_outputs
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    for output in &outputs {
        if authority.output_authorized(output)? {
            return unsound(format!("output {output} is already authorized"));
        }
    }
    authority.change(Change::Authorization {
        outputs,
        row: AuthorizationRow {
            decision: decision_key,
            seq: seq.get(),
            body: wire::encode(&authorization)?,
        },
    })
}

fn record<T: serde::Serialize>(
    authority: &mut impl Authority,
    seq: Seq,
    proposal: &ProposalRef,
    controller: String,
    body: &T,
) -> Result<(), Refusal> {
    let key = subject(proposal)?;
    if authority.proposal(&key)?.is_some() {
        return unsound(format!("proposal subject {key} is already recorded"));
    }
    authority.change(Change::Proposal {
        subject: key,
        row: ProposalRow {
            controller,
            seq: seq.get(),
            body: wire::encode(body)?,
        },
    })
}

fn require_task(authority: &impl Authority, task: &TaskStamp, role: &str) -> Result<(), Refusal> {
    match authority.task(task)? {
        Some(_) => Ok(()),
        None => unsound(format!("{role} task {task} does not exist")),
    }
}

fn require_proposal(
    authority: &impl Authority,
    proposal: &ProposalRef,
) -> Result<Vec<u8>, Refusal> {
    let key = subject(proposal)?;
    match authority.proposal(&key)? {
        Some(body) => Ok(body),
        None => unsound(format!("proposal subject {key} does not exist")),
    }
}

/// A link must name authority that exists before the linking fact. Artifact
/// bytes live outside the journal, so their digest is the whole link.
fn require_source(
    authority: &impl Authority,
    seq: Seq,
    source: &RoomSource,
) -> Result<(), Refusal> {
    match source {
        RoomSource::Journal { fact } => {
            if fact.get() == 0 || fact.get() >= seq.get() {
                return unsound(format!(
                    "source fact {} is not authority earlier than {seq}",
                    fact.get()
                ));
            }
        }
        RoomSource::Transcript(hunk) => require_hunk(authority, hunk)?,
        RoomSource::Artifact { .. } => {}
    }
    Ok(())
}

/// A hunk must name a nonempty text range of one decoded event inside the
/// accounted extent of its transcript generation.
fn require_hunk(authority: &impl Authority, hunk: &TranscriptHunk) -> Result<(), Refusal> {
    let TranscriptHunk { head, locus, text } = hunk;
    let Some(accounted) = authority.transcript(head)? else {
        return unsound(format!(
            "source transcript {} generation {} is not accounted",
            head.key().as_str(),
            head.source
        ));
    };
    if accounted.source != head.source {
        return unsound(format!(
            "source transcript {} generation {} is not accounted",
            head.key().as_str(),
            head.source
        ));
    }
    if !position_at_or_before(head.next, accounted.next) {
        return unsound(format!(
            "source transcript {} boundary {}/{} exceeds accounted boundary {}/{}",
            head.key().as_str(),
            head.next.member,
            head.next.offset,
            accounted.next.member,
            accounted.next.offset,
        ));
    }
    if !locus_within(*locus, head.next) {
        return unsound(format!(
            "source transcript {} locus {}/{}/{} exceeds linked boundary {}/{}",
            head.key().as_str(),
            locus.member,
            locus.offset,
            locus.length,
            head.next.member,
            head.next.offset,
        ));
    }
    if locus.event == u32::MAX || text.start >= text.end {
        return unsound(format!(
            "source transcript {} hunk {}..{} of event {} is not decoded event text",
            head.key().as_str(),
            text.start,
            text.end,
            locus.event,
        ));
    }
    Ok(())
}

fn position_at_or_before(
    position: wire::body::TranscriptPosition,
    boundary: wire::body::TranscriptPosition,
) -> bool {
    position.member < boundary.member
        || (position.member == boundary.member && position.offset <= boundary.offset)
}

fn locus_within(locus: wire::MessageLocus, boundary: wire::body::TranscriptPosition) -> bool {
    if locus.length == 0 {
        return false;
    }
    let Some(end) = locus.offset.checked_add(locus.length) else {
        return false;
    };
    locus.member < boundary.member || (locus.member == boundary.member && end <= boundary.offset)
}
