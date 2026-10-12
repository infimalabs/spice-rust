//! Room conversation authority: source-backed MSG, ASK, and ANS publications,
//! one guarded operator item per operator delivery, and one guarded result
//! head per ASK.
//!
//! Lane deliveries are native steering directives, never room heads: each lane
//! route, and each answer returned to a lane requester, names one directive its
//! own write opened, and an ASK's directive is keyed by the ASK's result handle.
//! That directive's ACK, lapse, recovery, dead-letter, and cancellation move
//! only the directive; its NAK also declines the ASK's pending result in the
//! same fact. Only an operator delivery opens an operator item here.
//!
//! A publication is write-once. An ASK opens a result that only one terminal
//! outcome settles, before its immutable deadline or by timing out at it. The
//! fold compares every ANS with the commit instant, so a late answer refuses
//! even before any timeout is recorded, while an admitted answer replays at its
//! own instant. Answer admission, retirement of the live ASK delivery, and the
//! answer's return delivery commit in one write. Every fact computes all of its
//! edits before applying any, so a refused fact leaves each result and item as
//! it was. Item lapse and recovery move only the item, and controls open
//! nothing.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use wire::body::{SteerAcked, SteerDeclined, SteerMessaged, SteerSent};
use wire::rooms::{
    QuestionClosure, QuestionParties, QuestionResultHead, QuestionResultRecord, QuestionResultRef,
    QuestionResultState, RoomAnchor, RoomConversed, RoomDelivery, RoomDestination, RoomDisposed,
    RoomPosted, RoomPublication, RoomPublicationKind, RoomQuestionClosed, RoomQuestionTimedOut,
    RoomReceiptControl, RoomReceiptHead, RoomReceiptLapsed, RoomReceiptRecord,
    RoomReceiptRecovered, RoomReceiptRef, RoomReceiptState, RoomSource, TranscriptHunk,
};
use wire::{Actor, AnyFact, CanonicalUuid, Disposition, Instant, Seq, SequenceCode};

use super::{Authority, Change, Refusal, require_hunk, require_operator, require_task, unsound};

/// Conversation publications and their result heads, created with the
/// current schema.
pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE room_conversations (
    publication TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE room_results (
    result TEXT PRIMARY KEY,
    requester TEXT NOT NULL,
    pending INTEGER NOT NULL CHECK (pending IN (0, 1)),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    record BLOB NOT NULL,
    head BLOB NOT NULL
) STRICT;
CREATE INDEX room_results_pending ON room_results(requester, pending);
"#;

/// Operator item heads, created with the current schema.
pub(crate) const ITEM_SCHEMA_SQL: &str = r#"
CREATE TABLE room_items (
    item TEXT PRIMARY KEY,
    open INTEGER NOT NULL CHECK (open IN (0, 1)),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    record BLOB NOT NULL,
    head BLOB NOT NULL
) STRICT;
CREATE INDEX room_items_open ON room_items(open, head_seq);
"#;

/// One stored operator item head: whether it still owes a disposition, the
/// position of its latest move, and its canonical record and head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ItemRow {
    pub(crate) open: bool,
    pub(crate) seq: u64,
    pub(crate) record: Vec<u8>,
    pub(crate) head: Vec<u8>,
}

/// One stored result head: its requester, whether it is still pending, the
/// position of its latest move, and its canonical record and head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResultRow {
    pub(crate) requester: String,
    pub(crate) pending: bool,
    pub(crate) seq: u64,
    pub(crate) record: Vec<u8>,
    pub(crate) head: Vec<u8>,
}

/// The conversation projection, row for row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Heads {
    /// Publication to its admitting position and canonical body.
    pub(crate) publications: BTreeMap<String, (u64, Vec<u8>)>,
    /// Operator item key to its head.
    pub(crate) items: BTreeMap<String, ItemRow>,
    /// Result handle to its head.
    pub(crate) results: BTreeMap<String, ResultRow>,
}

/// One conversation projection consequence of an admitted fact.
pub(crate) enum Edit {
    Publication {
        key: String,
        seq: u64,
        body: Vec<u8>,
    },
    Item {
        key: String,
        row: ItemRow,
    },
    Result {
        key: String,
        row: ResultRow,
    },
}

impl Heads {
    pub(super) fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::Publication { key, seq, body } => {
                self.publications.insert(key, (seq, body));
            }
            Edit::Item { key, row } => {
                self.items.insert(key, row);
            }
            Edit::Result { key, row } => {
                self.results.insert(key, row);
            }
        }
    }

    pub(super) fn pending_results(&self, requester: &str) -> usize {
        self.results
            .values()
            .filter(|row| row.pending && row.requester == requester)
            .count()
    }

    pub(super) fn load(connection: &Connection) -> Result<Self, rusqlite::Error> {
        let mut heads = Self::default();
        let mut statement =
            connection.prepare("SELECT publication, head_seq, body FROM room_conversations")?;
        for row in statement.query_map([], |row| {
            Ok((row.get(0)?, (super::sql::unsql(row.get(1)?), row.get(2)?)))
        })? {
            let (key, value) = row?;
            heads.publications.insert(key, value);
        }
        let mut statement =
            connection.prepare("SELECT item, open, head_seq, record, head FROM room_items")?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                ItemRow {
                    open: row.get(1)?,
                    seq: super::sql::unsql(row.get(2)?),
                    record: row.get(3)?,
                    head: row.get(4)?,
                },
            ))
        })? {
            let (key, value) = row?;
            heads.items.insert(key, value);
        }
        let mut statement = connection.prepare(
            "SELECT result, requester, pending, head_seq, record, head FROM room_results",
        )?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                ResultRow {
                    requester: row.get(1)?,
                    pending: row.get(2)?,
                    seq: super::sql::unsql(row.get(3)?),
                    record: row.get(4)?,
                    head: row.get(5)?,
                },
            ))
        })? {
            let (key, value) = row?;
            heads.results.insert(key, value);
        }
        Ok(heads)
    }

    /// Replace every stored conversation row with these heads.
    pub(super) fn write(&self, connection: &Connection) -> Result<(), rusqlite::Error> {
        connection.execute_batch(
            "DELETE FROM room_conversations; DELETE FROM room_items; DELETE FROM room_results;",
        )?;
        for (key, (seq, body)) in &self.publications {
            apply_sql(
                connection,
                Edit::Publication {
                    key: key.clone(),
                    seq: *seq,
                    body: body.clone(),
                },
            )?;
        }
        for (key, row) in &self.items {
            apply_sql(
                connection,
                Edit::Item {
                    key: key.clone(),
                    row: row.clone(),
                },
            )?;
        }
        for (key, row) in &self.results {
            apply_sql(
                connection,
                Edit::Result {
                    key: key.clone(),
                    row: row.clone(),
                },
            )?;
        }
        Ok(())
    }
}

/// Write one edit into the stored projection.
pub(super) fn apply_sql(connection: &Connection, edit: Edit) -> Result<(), rusqlite::Error> {
    match edit {
        Edit::Publication { key, seq, body } => {
            connection.execute(
                "INSERT INTO room_conversations(publication, head_seq, body) VALUES (?1, ?2, ?3)",
                params![key, super::sql::sql(seq), body],
            )?;
        }
        Edit::Item { key, row } => {
            connection.execute(
                "INSERT INTO room_items(item, open, head_seq, record, head) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(item) DO UPDATE SET \
                    open = excluded.open, head_seq = excluded.head_seq, head = excluded.head",
                params![
                    key,
                    row.open,
                    super::sql::sql(row.seq),
                    row.record,
                    row.head
                ],
            )?;
        }
        Edit::Result { key, row } => {
            connection.execute(
                "INSERT INTO room_results(result, requester, pending, head_seq, record, head) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(result) DO UPDATE SET \
                    pending = excluded.pending, head_seq = excluded.head_seq, head = excluded.head",
                params![
                    key,
                    row.requester,
                    row.pending,
                    super::sql::sql(row.seq),
                    row.record,
                    row.head
                ],
            )?;
        }
    }
    Ok(())
}

/// Stored publication body, as one commit transaction sees it.
pub(super) fn stored_publication(
    connection: &Connection,
    key: &str,
) -> Result<Option<Vec<u8>>, Refusal> {
    Ok(connection
        .query_row(
            "SELECT body FROM room_conversations WHERE publication = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

/// Stored operator item head, as one commit transaction sees it.
pub(super) fn stored_item(connection: &Connection, key: &str) -> Result<Option<ItemRow>, Refusal> {
    Ok(connection
        .query_row(
            "SELECT open, head_seq, record, head FROM room_items WHERE item = ?1",
            [key],
            |row| {
                Ok(ItemRow {
                    open: row.get(0)?,
                    seq: super::sql::unsql(row.get(1)?),
                    record: row.get(2)?,
                    head: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// Stored result head, as one commit transaction sees it.
pub(super) fn stored_result(
    connection: &Connection,
    key: &str,
) -> Result<Option<ResultRow>, Refusal> {
    Ok(connection
        .query_row(
            "SELECT requester, pending, head_seq, record, head FROM room_results WHERE result = ?1",
            [key],
            |row| {
                Ok(ResultRow {
                    requester: row.get(0)?,
                    pending: row.get(1)?,
                    seq: super::sql::unsql(row.get(2)?),
                    record: row.get(3)?,
                    head: row.get(4)?,
                })
            },
        )
        .optional()?)
}

/// Stored count of one requester's pending results.
pub(super) fn stored_pending_results(
    connection: &Connection,
    requester: &str,
) -> Result<usize, Refusal> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM room_results WHERE requester = ?1 AND pending = 1",
        [requester],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(count).unwrap_or_default())
}

/// Canonical key of one route destination.
pub(crate) fn destination_key(destination: &RoomDestination) -> Result<String, Refusal> {
    String::from_utf8(wire::encode(destination)?)
        .map_err(|_| Refusal::Unsound("room destination key is not text".to_owned()))
}

/// Admit one conversation fact, or `None` when `fact` is not one.
pub(super) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    let edits = if let Some(body) = fact.read::<RoomConversed>() {
        conversed(authority, seq, at, actor, &body)
    } else if let Some(body) = fact.read::<RoomPosted>() {
        require_operator(actor, "an operator room post").and_then(|()| {
            let publication = body.publication(code(seq)).or_else(unsound)?;
            admit(
                authority,
                seq,
                at,
                &publication,
                &body.directed,
                body.expires,
            )
        })
    } else if let Some(body) = fact.read::<SteerDeclined>() {
        directive_declined(authority, seq, &body)
    } else if let Some(body) = fact.read::<RoomDisposed>() {
        require_operator(actor, "an operator ACK or NAK")
            .and_then(|()| body.control(code(seq)).or_else(unsound))
            .and_then(|control| controlled(authority, seq, &control))
    } else if let Some(body) = fact.read::<RoomQuestionClosed>() {
        closed(authority, seq, actor, &body)
    } else if let Some(body) = fact.read::<RoomQuestionTimedOut>() {
        timed_out(authority, seq, at, actor, &body.result)
    } else if let Some(body) = fact.read::<RoomReceiptLapsed>() {
        lapsed(authority, seq, at, actor, &body.receipt)
    } else if let Some(body) = fact.read::<RoomReceiptRecovered>() {
        recovered(authority, seq, at, actor, &body)
    } else {
        return None;
    };
    Some(edits.and_then(|edits| {
        for edit in edits {
            authority.change(Change::Conversation(edit))?;
        }
        Ok(())
    }))
}

fn conversed(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    body: &RoomConversed,
) -> Result<Vec<Edit>, Refusal> {
    require_lane(actor, body.hunk.head.lane, "a room conversation")?;
    require_hunk(authority, &body.hunk)?;
    let publication = body.publication().or_else(unsound)?;
    admit(
        authority,
        seq,
        at,
        &publication,
        &body.directed,
        body.expires,
    )
}

/// Admit one substantive publication with the operator item and result it
/// opens, bound to the directives its write opened for its lane deliveries.
fn admit(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    publication: &RoomPublication,
    directed: &[CanonicalUuid],
    expires: Instant,
) -> Result<Vec<Edit>, Refusal> {
    let stamp = publication.publication();
    if authority.conversation(&stamp.to_string())?.is_some() {
        return unsound(format!("room publication {stamp} is already admitted"));
    }
    if directed
        .iter()
        .enumerate()
        .any(|(index, directive)| directed[index + 1..].contains(directive))
    {
        return unsound(format!(
            "room publication {stamp} names one directive twice"
        ));
    }
    for route in publication.routes() {
        if let RoomDestination::Task { task } = &route.destination {
            require_task(authority, task, "room route")?;
        }
        if let Some(context) = &route.context {
            require_task(authority, context, "room route context")?;
        }
    }
    let active = match publication.kind() {
        RoomPublicationKind::Ask { .. } => {
            authority.pending_results(&destination_key(publication.author())?)?
        }
        RoomPublicationKind::Msg {} | RoomPublicationKind::Ans { .. } => 0,
    };
    if let Err(error) = publication.admission_charge(active) {
        return unsound(format!("room publication {stamp}: {error}"));
    }
    if !matches!(publication.kind(), RoomPublicationKind::Ans { .. })
        && directed.len() != publication.lane_routes().count()
    {
        return unsound(format!(
            "room publication {stamp} must name one directive for each lane route"
        ));
    }
    let mut edits = match publication.kind() {
        RoomPublicationKind::Msg {} => Vec::new(),
        RoomPublicationKind::Ask { result, deadline } => {
            vec![asked(authority, seq, at, publication, result, *deadline)?]
        }
        RoomPublicationKind::Ans { result, .. } => {
            answered(authority, seq, at, publication, result, directed, expires)?
        }
    };
    if let Some((item, context)) = publication.operator_item() {
        edits.push(open(
            authority,
            seq,
            at,
            item,
            publication,
            context.cloned(),
            expires,
        )?);
    }
    edits.push(Edit::Publication {
        key: stamp.to_string(),
        seq: seq.get(),
        body: wire::encode(publication)?,
    });
    Ok(edits)
}

fn asked(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    publication: &RoomPublication,
    result: &QuestionResultRef,
    deadline: Instant,
) -> Result<Edit, Refusal> {
    if deadline <= at {
        return unsound(format!(
            "question {result} deadline must follow its admission"
        ));
    }
    if authority.question_result(result.as_str())?.is_some() {
        return unsound(format!("question result {result} already exists"));
    }
    let route = &publication.routes()[0];
    let record = QuestionResultRecord::try_new(
        result.clone(),
        publication.publication().clone(),
        QuestionParties {
            requester: publication.author().clone(),
            respondent: route.destination.clone(),
            context: route.context.clone(),
        },
        publication.source().clone(),
        deadline,
    )
    .or_else(unsound)?;
    Question {
        head: QuestionResultHead::pending(result.clone()),
        record,
    }
    .edit(seq)
}

/// Settle the answered result, retire a live operator ASK item, and open the
/// answer's return item to an operator requester, together. A lane
/// respondent's live ASK directive and a lane requester's return directive are
/// bound to this write by [`require_deliveries`].
fn answered(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    publication: &RoomPublication,
    result: &QuestionResultRef,
    directed: &[CanonicalUuid],
    expires: Instant,
) -> Result<Vec<Edit>, Refusal> {
    let mut question = Question::load(authority, result)?;
    let state = question
        .record
        .answer_state(publication, at, directed)
        .or_else(|error| unsound(format!("ANS to question {result}: {error}")))?;
    let QuestionResultState::Answered {
        return_delivery, ..
    } = &state
    else {
        return unsound(format!("ANS to question {result} did not answer it"));
    };
    let returned = return_delivery.delivery().clone();
    question.settle(state)?;
    let mut edits = vec![question.edit(seq)?];
    if let Some(item) = question.record.item() {
        let mut asked = Item::load(authority, &item)?;
        if matches!(asked.head.state(), RoomReceiptState::Pending { .. }) {
            asked
                .head
                .settle(RoomReceiptState::Acked {
                    source: publication.source().clone(),
                })
                .or_else(|error| unsound(error.to_string()))?;
            edits.push(asked.edit(seq)?);
        }
    }
    if let RoomDelivery::Item { item } = returned {
        edits.push(open(
            authority,
            seq,
            at,
            item,
            publication,
            question.record.context().cloned(),
            expires,
        )?);
    }
    Ok(edits)
}

/// A lane's NAK of a live ASK directive declines that ASK's pending result
/// in the same fact. Any other decline, including one after the result
/// settled, moves only native steering.
fn directive_declined(
    authority: &impl Authority,
    seq: Seq,
    body: &SteerDeclined,
) -> Result<Vec<Edit>, Refusal> {
    let Some(row) = authority.question_result(&body.key)? else {
        return Ok(Vec::new());
    };
    let mut question = Question {
        record: wire::decode(&row.record)?,
        head: wire::decode(&row.head)?,
    };
    if question.record.respondent() != (&RoomDestination::Lane { lane: body.lane })
        || question.head.state().is_terminal()
    {
        return Ok(Vec::new());
    }
    question.settle(QuestionResultState::Declined {
        source: RoomSource::Journal { fact: code(seq) },
    })?;
    Ok(vec![question.edit(seq)?])
}

/// Settle every operator item an ACK or NAK names, and decline an operator
/// ASK's pending result on NAK even after its item retired.
fn controlled(
    authority: &impl Authority,
    seq: Seq,
    control: &RoomReceiptControl,
) -> Result<Vec<Edit>, Refusal> {
    let mut edits = Vec::new();
    for key in control.keys() {
        let mut item = Item::load(authority, key)?;
        let mut question = match authority.question_result(&key.to_string())? {
            Some(row) => Some(Question {
                record: wire::decode(&row.record)?,
                head: wire::decode(&row.head)?,
            }),
            None => None,
        }
        .filter(|question| question.record.item().as_ref() == Some(key));
        let before = item.head.state().clone();
        let settled = item.head.settle(control.receipt_state());
        let declined = question.as_ref().and_then(|question| {
            matches!(question.head.state(), QuestionResultState::Pending {})
                .then(|| question.record.control_result(control))
                .flatten()
        });
        match settled {
            Ok(()) if item.head.state() != &before => edits.push(item.edit(seq)?),
            _ if declined.is_some() => {}
            Ok(()) => {
                return unsound(format!(
                    "operator item {} already carries this disposition",
                    item.key
                ));
            }
            Err(error) => return unsound(format!("operator item {}: {error}", item.key)),
        }
        if let (Some(question), Some(state)) = (question.as_mut(), declined) {
            question.settle(state)?;
            edits.push(question.edit(seq)?);
        }
    }
    Ok(edits)
}

/// An explicit decline by the respondent or cancellation by the requester,
/// either of which the operator may override. A pending operator ASK item
/// retires with it; a lane respondent's directive stays with native steering.
fn closed(
    authority: &impl Authority,
    seq: Seq,
    actor: &Actor,
    body: &RoomQuestionClosed,
) -> Result<Vec<Edit>, Refusal> {
    let mut question = Question::load(authority, &body.result)?;
    let source = RoomSource::Journal { fact: code(seq) };
    let (party, item_state) = match body.closure {
        QuestionClosure::Declined => (
            question.record.respondent(),
            RoomReceiptState::Declined { source },
        ),
        QuestionClosure::Cancelled => (
            question.record.requester(),
            RoomReceiptState::Cancelled { source },
        ),
    };
    if *actor != Actor::Operator && !acts_as(actor, party) {
        return unsound(format!(
            "{actor} cannot close question {} for its stored party",
            body.result
        ));
    }
    question.settle(body.state(code(seq)))?;
    let mut edits = vec![question.edit(seq)?];
    if let Some(item) = question.record.item() {
        let mut asked = Item::load(authority, &item)?;
        if matches!(asked.head.state(), RoomReceiptState::Pending { .. }) {
            asked
                .head
                .settle(item_state)
                .or_else(|error| unsound(error.to_string()))?;
            edits.push(asked.edit(seq)?);
        }
    }
    Ok(edits)
}

fn timed_out(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    result: &QuestionResultRef,
) -> Result<Vec<Edit>, Refusal> {
    require_resident(actor, "a question timeout")?;
    let mut question = Question::load(authority, result)?;
    if at < question.record.deadline() {
        return unsound(format!("question {result} deadline has not passed"));
    }
    if question.head.state().is_terminal() {
        return unsound(format!("question result {result} is already settled"));
    }
    question.settle(QuestionResultState::TimedOut {})?;
    Ok(vec![question.edit(seq)?])
}

fn lapsed(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    receipt: &RoomReceiptRef,
) -> Result<Vec<Edit>, Refusal> {
    require_resident(actor, "an operator item lapse")?;
    let mut item = Item::load(authority, receipt)?;
    let next = match item.head.state() {
        RoomReceiptState::Pending { expires } if at >= *expires => RoomReceiptState::Expired { at },
        RoomReceiptState::Pending { .. } => {
            return unsound(format!("operator item {} has not expired", item.key));
        }
        _ => return unsound(format!("operator item {} is not pending", item.key)),
    };
    item.head
        .settle(next)
        .or_else(|error| unsound(error.to_string()))?;
    Ok(vec![item.edit(seq)?])
}

fn recovered(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    body: &RoomReceiptRecovered,
) -> Result<Vec<Edit>, Refusal> {
    require_operator(actor, "operator item recovery")?;
    let mut item = Item::load(authority, &body.receipt)?;
    if body.expires <= at {
        return unsound(format!(
            "recovered operator item {} must expire after its recovery",
            item.key
        ));
    }
    item.head
        .recover(body.expires)
        .or_else(|error| unsound(format!("operator item {}: {error}", item.key)))?;
    Ok(vec![item.edit(seq)?])
}

/// Open one pending operator item that `publication` delivers.
fn open(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    item: RoomReceiptRef,
    publication: &RoomPublication,
    context: Option<RoomAnchor>,
    expires: Instant,
) -> Result<Edit, Refusal> {
    let key = item.to_string();
    if authority.room_item(&key)?.is_some() {
        return unsound(format!("operator item {key} already exists"));
    }
    if expires <= at {
        return unsound(format!(
            "operator item {key} must expire after its admission"
        ));
    }
    Item {
        key,
        record: RoomReceiptRecord::new(
            item.clone(),
            publication.publication().clone(),
            context,
            publication.source().clone(),
        ),
        head: RoomReceiptHead::pending(item, expires),
    }
    .edit(seq)
}

/// One decoded operator item head and the immutable record behind it.
struct Item {
    key: String,
    record: RoomReceiptRecord,
    head: RoomReceiptHead,
}

impl Item {
    fn load(authority: &impl Authority, item: &RoomReceiptRef) -> Result<Self, Refusal> {
        let key = item.to_string();
        let Some(row) = authority.room_item(&key)? else {
            return unsound(format!("operator item {key} does not exist"));
        };
        Ok(Self {
            record: wire::decode(&row.record)?,
            head: wire::decode(&row.head)?,
            key,
        })
    }

    fn edit(&self, seq: Seq) -> Result<Edit, Refusal> {
        Ok(Edit::Item {
            key: self.key.clone(),
            row: ItemRow {
                open: !self.head.state().is_terminal(),
                seq: seq.get(),
                record: wire::encode(&self.record)?,
                head: wire::encode(&self.head)?,
            },
        })
    }
}

/// One decoded result head and the immutable question record behind it.
struct Question {
    record: QuestionResultRecord,
    head: QuestionResultHead,
}

impl Question {
    fn load(authority: &impl Authority, result: &QuestionResultRef) -> Result<Self, Refusal> {
        let Some(row) = authority.question_result(result.as_str())? else {
            return unsound(format!("question result {result} does not exist"));
        };
        Ok(Self {
            record: wire::decode(&row.record)?,
            head: wire::decode(&row.head)?,
        })
    }

    fn settle(&mut self, state: QuestionResultState) -> Result<(), Refusal> {
        let result = self.record.result();
        self.head
            .settle(state)
            .or_else(|error| unsound(format!("question result {result}: {error}")))
    }

    fn edit(&self, seq: Seq) -> Result<Edit, Refusal> {
        Ok(Edit::Result {
            key: self.record.result().to_string(),
            row: ResultRow {
                requester: destination_key(self.record.requester())?,
                pending: !self.head.state().is_terminal(),
                seq: seq.get(),
                record: wire::encode(&self.record)?,
                head: wire::encode(&self.head)?,
            },
        })
    }
}

/// One directive a write opens, as a conversation's lane delivery reads it.
struct Opened {
    directive: CanonicalUuid,
    key: String,
    lane: CanonicalUuid,
    room: Option<RoomAnchor>,
    /// The packet that told it, for a peer message; absent for operator direction.
    told: Option<SteerMessaged>,
    from: Option<CanonicalUuid>,
}

impl Opened {
    fn read(fact: &AnyFact) -> Option<Self> {
        if let Some(message) = fact.read::<SteerMessaged>() {
            return Some(Self {
                directive: message.directive,
                key: message.key.clone(),
                lane: message.lane,
                room: message.room.clone(),
                from: Some(message.from),
                told: Some(message),
            });
        }
        fact.read::<SteerSent>().map(|sent| Self {
            directive: sent.directive,
            key: sent.key,
            lane: sent.lane,
            room: sent.room,
            told: None,
            from: sent.from,
        })
    }

    /// Whether a lane author's packet at `hunk` told this peer message.
    fn told_by(&self, hunk: &TranscriptHunk) -> bool {
        self.told.as_ref().is_some_and(|message| {
            let locus = &message.locus;
            message.from == hunk.head.lane
                && locus.source == Some(hunk.head.source)
                && (locus.member, locus.offset, locus.length, locus.event)
                    == (
                        hunk.locus.member,
                        hunk.locus.offset,
                        hunk.locus.length,
                        hunk.locus.event,
                    )
        })
    }
}

/// Each conversation in a write names, in order, exactly the native
/// directives that write opened for its lane deliveries: a peer message told
/// from a lane author's own record, or operator direction for an operator
/// author. Each goes to its lane with that route's context; an ASK's directive
/// carries the ASK's result handle as its key, and an answer's return carries
/// the stored question's context. An answer by a lane respondent whose ASK
/// directive is still live acknowledges that directive in the same write.
///
/// The fold has already admitted the conversation, so an answer's question is
/// stored; native steering heads are not yet applied, so they still show the
/// standing before this write.
pub(crate) fn require_deliveries<'a>(
    connection: &Connection,
    facts: impl IntoIterator<Item = &'a AnyFact>,
) -> Result<(), Refusal> {
    let facts = facts.into_iter().collect::<Vec<_>>();
    let opened = facts
        .iter()
        .filter_map(|fact| Opened::read(fact))
        .collect::<Vec<_>>();
    for fact in &facts {
        let (stamp, hunk, operation, routes, directed) =
            if let Some(body) = fact.read::<RoomConversed>() {
                (
                    body.publication,
                    Some(body.hunk),
                    body.operation,
                    body.routes,
                    body.directed,
                )
            } else if let Some(body) = fact.read::<RoomPosted>() {
                (
                    body.publication,
                    None,
                    body.operation,
                    body.routes,
                    body.directed,
                )
            } else {
                continue;
            };
        let lanes = routes.iter().filter_map(|route| match route.destination {
            RoomDestination::Lane { lane } => Some((lane, route.context.clone())),
            _ => None,
        });
        let expected = match &operation {
            RoomPublicationKind::Msg {} => lanes.map(|(lane, room)| (lane, room, None)).collect(),
            RoomPublicationKind::Ask { result, .. } => lanes
                .map(|(lane, room)| (lane, room, Some(result.as_str())))
                .collect(),
            RoomPublicationKind::Ans { result, .. } => {
                let Some(row) = stored_result(connection, result.as_str())? else {
                    return unsound(format!("ANS {stamp} names no stored question {result}"));
                };
                let record: QuestionResultRecord = wire::decode(&row.record)?;
                if matches!(record.respondent(), RoomDestination::Lane { .. })
                    && live_directive(connection, result.as_str())?
                    && !facts.iter().any(|fact| {
                        fact.read::<SteerAcked>()
                            .is_some_and(|acked| acked.key == result.as_str())
                    })
                {
                    return unsound(format!(
                        "ANS {stamp} leaves the live ASK directive {result} unretired"
                    ));
                }
                match record.requester() {
                    RoomDestination::Lane { lane } => {
                        vec![(*lane, record.context().cloned(), None)]
                    }
                    _ => Vec::new(),
                }
            }
        };
        if directed.len() != expected.len() {
            return unsound(format!(
                "room publication {stamp} names other lane deliveries than its routes need"
            ));
        }
        for (directive, (lane, room, key)) in directed.iter().zip(expected) {
            let Some(open) = opened.iter().find(|open| open.directive == *directive) else {
                return unsound(format!(
                    "room publication {stamp} names directive {directive} its write did not open"
                ));
            };
            let authored = match &hunk {
                Some(hunk) => open.told_by(hunk),
                None => open.told.is_none() && open.from.is_none(),
            };
            if !authored
                || open.lane != lane
                || open.room != room
                || key.is_some_and(|key| open.key != key)
            {
                return unsound(format!(
                    "directive {directive} is not room publication {stamp}'s delivery to lane {lane}"
                ));
            }
        }
    }
    Ok(())
}

/// Whether a native directive with this key is live before the current write.
fn live_directive(connection: &Connection, key: &str) -> Result<bool, Refusal> {
    let body: Option<Vec<u8>> = connection
        .query_row(
            "SELECT body FROM steering_heads WHERE steer_key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(match body {
        Some(body) => wire::decode::<wire::SteeringHead>(&body)?.disposition == Disposition::Live,
        None => false,
    })
}

fn code(seq: Seq) -> SequenceCode {
    SequenceCode::new(seq.get())
}

fn acts_as(actor: &Actor, party: &RoomDestination) -> bool {
    match (actor, party) {
        (Actor::Operator, RoomDestination::Operator {}) => true,
        (Actor::Lane(actor), RoomDestination::Lane { lane }) => actor == lane,
        _ => false,
    }
}

fn require_lane(actor: &Actor, lane: CanonicalUuid, what: &str) -> Result<(), Refusal> {
    if *actor == Actor::Lane(lane) {
        return Ok(());
    }
    unsound(format!("{what} from lane {lane} is written by {actor}"))
}

fn require_resident(actor: &Actor, what: &str) -> Result<(), Refusal> {
    if *actor == Actor::Spice {
        return Ok(());
    }
    unsound(format!("{what} is resident-owned, not written by {actor}"))
}
