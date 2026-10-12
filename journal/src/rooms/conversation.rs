//! Room conversation authority: source-backed MSG, ASK, and ANS publications,
//! one guarded receipt head per directed route, and one guarded result head
//! per ASK.
//!
//! A publication is write-once. Each operator or lane route it directs opens a
//! receipt pending until a bounded expiry; an ASK also opens a result that only
//! one terminal outcome settles, before its immutable deadline or by timing out
//! at it. The fold compares every ANS with the commit instant, so a late answer
//! refuses even before any timeout is recorded, while an admitted answer
//! replays at its own instant. Answer admission, retirement of a live ASK
//! receipt, and the answer's return receipt land in one fact. Every fact
//! computes all of its edits before applying any, so a refused fact leaves each
//! result and receipt as it was. Receipt lapse and recovery move only the
//! receipt, and control returns open nothing.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use wire::rooms::{
    QuestionClosure, QuestionParties, QuestionResultHead, QuestionResultRecord, QuestionResultRef,
    QuestionResultState, RoomConversed, RoomDeliveryKey, RoomDestination, RoomDisposed, RoomPosted,
    RoomPublication, RoomPublicationKind, RoomQuestionClosed, RoomQuestionTimedOut,
    RoomReceiptControl, RoomReceiptHead, RoomReceiptLapsed, RoomReceiptRecord,
    RoomReceiptRecovered, RoomReceiptRef, RoomReceiptState, RoomReceiptUnavailable, RoomReceipted,
    RoomRoute, RoomSource,
};
use wire::{Actor, AnyFact, Instant, Seq, SequenceCode};

use super::{Authority, Change, Refusal, require_hunk, require_operator, require_task, unsound};

/// Conversation publications with their receipt and result heads, created
/// with the current schema.
pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE room_conversations (
    publication TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE room_receipts (
    receipt TEXT PRIMARY KEY,
    destination TEXT NOT NULL,
    open INTEGER NOT NULL CHECK (open IN (0, 1)),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    record BLOB NOT NULL,
    head BLOB NOT NULL
) STRICT;
CREATE INDEX room_receipts_open ON room_receipts(destination, open, head_seq);
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

/// One stored receipt head: its destination, whether it still owes a
/// disposition, the position of its latest move, and its canonical record and
/// head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReceiptRow {
    pub(crate) destination: String,
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
    /// Canonical receipt reference to its head.
    pub(crate) receipts: BTreeMap<String, ReceiptRow>,
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
    Receipt {
        key: String,
        row: ReceiptRow,
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
            Edit::Receipt { key, row } => {
                self.receipts.insert(key, row);
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
        let mut statement = connection.prepare(
            "SELECT receipt, destination, open, head_seq, record, head FROM room_receipts",
        )?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                ReceiptRow {
                    destination: row.get(1)?,
                    open: row.get(2)?,
                    seq: super::sql::unsql(row.get(3)?),
                    record: row.get(4)?,
                    head: row.get(5)?,
                },
            ))
        })? {
            let (key, value) = row?;
            heads.receipts.insert(key, value);
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
            "DELETE FROM room_conversations; DELETE FROM room_receipts; DELETE FROM room_results;",
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
        for (key, row) in &self.receipts {
            apply_sql(
                connection,
                Edit::Receipt {
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
        Edit::Receipt { key, row } => {
            connection.execute(
                "INSERT INTO room_receipts(receipt, destination, open, head_seq, record, head) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(receipt) DO UPDATE SET \
                    open = excluded.open, head_seq = excluded.head_seq, head = excluded.head",
                params![
                    key,
                    row.destination,
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

/// Stored receipt head, as one commit transaction sees it.
pub(super) fn stored_receipt(
    connection: &Connection,
    key: &str,
) -> Result<Option<ReceiptRow>, Refusal> {
    Ok(connection
        .query_row(
            "SELECT destination, open, head_seq, record, head FROM room_receipts WHERE receipt = ?1",
            [key],
            |row| {
                Ok(ReceiptRow {
                    destination: row.get(0)?,
                    open: row.get(1)?,
                    seq: super::sql::unsql(row.get(2)?),
                    record: row.get(3)?,
                    head: row.get(4)?,
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

/// Canonical key of one receipt reference.
pub(crate) fn receipt_key(receipt: &RoomReceiptRef) -> Result<String, Refusal> {
    canonical(receipt)
}

/// Canonical key of one route destination.
pub(crate) fn destination_key(destination: &RoomDestination) -> Result<String, Refusal> {
    canonical(destination)
}

fn canonical<T: serde::Serialize>(value: &T) -> Result<String, Refusal> {
    String::from_utf8(wire::encode(value)?)
        .map_err(|_| Refusal::Unsound("room conversation key is not text".to_owned()))
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
            admit(authority, seq, at, &publication, body.expires)
        })
    } else if let Some(body) = fact.read::<RoomReceipted>() {
        let lane = body.hunk.head.lane;
        require_lane(actor, lane, "an ACK or NAK")
            .and_then(|()| require_hunk(authority, &body.hunk))
            .and_then(|()| body.control().or_else(unsound))
            .and_then(|control| {
                controlled(authority, seq, &RoomDestination::Lane { lane }, &control)
            })
    } else if let Some(body) = fact.read::<RoomDisposed>() {
        require_operator(actor, "an operator ACK or NAK")
            .and_then(|()| body.control(code(seq)).or_else(unsound))
            .and_then(|control| controlled(authority, seq, &RoomDestination::Operator {}, &control))
    } else if let Some(body) = fact.read::<RoomQuestionClosed>() {
        closed(authority, seq, actor, &body)
    } else if let Some(body) = fact.read::<RoomQuestionTimedOut>() {
        timed_out(authority, seq, at, actor, &body.result)
    } else if let Some(body) = fact.read::<RoomReceiptLapsed>() {
        lapsed(authority, seq, at, actor, &body.receipt, false)
    } else if let Some(body) = fact.read::<RoomReceiptUnavailable>() {
        lapsed(authority, seq, at, actor, &body.receipt, true)
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
    admit(authority, seq, at, &publication, body.expires)
}

/// Admit one substantive publication with every receipt and result it opens.
fn admit(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    publication: &RoomPublication,
    expires: Instant,
) -> Result<Vec<Edit>, Refusal> {
    let stamp = publication.publication();
    if authority.conversation(&stamp.to_string())?.is_some() {
        return unsound(format!("room publication {stamp} is already admitted"));
    }
    if expires <= at {
        return unsound(format!(
            "room publication {stamp} receipts must expire after admission"
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
    let mut edits = match publication.kind() {
        RoomPublicationKind::Msg {} => publication
            .routes()
            .iter()
            .enumerate()
            .filter(|(_, route)| route.destination.is_audience())
            .map(|(index, route)| open(seq, publication, index, route.clone(), expires))
            .collect::<Result<Vec<_>, _>>()?,
        RoomPublicationKind::Ask { result, deadline } => {
            asked(authority, seq, at, publication, result, *deadline, expires)?
        }
        RoomPublicationKind::Ans { result, .. } => {
            answered(authority, seq, at, publication, result, expires)?
        }
    };
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
    expires: Instant,
) -> Result<Vec<Edit>, Refusal> {
    if deadline <= at {
        return unsound(format!(
            "question {result} deadline must follow its admission"
        ));
    }
    if authority.question_result(&result.to_string())?.is_some() {
        return unsound(format!("question result {result} already exists"));
    }
    let route = &publication.routes()[0];
    let receipt = open(seq, publication, 0, route.clone(), expires)?;
    let record = QuestionResultRecord::try_new(
        result.clone(),
        publication.publication().clone(),
        receipt_ref(publication, 0),
        QuestionParties {
            requester: publication.author().clone(),
            respondent: route.destination.clone(),
            context: route.context.clone(),
        },
        publication.source().clone(),
        deadline,
    )
    .or_else(unsound)?;
    let question = Question {
        head: QuestionResultHead::pending(result.clone()),
        record,
    };
    Ok(vec![receipt, question.edit(seq)?])
}

/// Settle the answered result, retire a live ASK receipt, and open the
/// answer's return receipt to the stored requester, together.
fn answered(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    publication: &RoomPublication,
    result: &QuestionResultRef,
    expires: Instant,
) -> Result<Vec<Edit>, Refusal> {
    let mut question = Question::load(authority, result)?;
    let returned = receipt_ref(publication, 0);
    let state = question
        .record
        .answer_state(publication, at, returned)
        .or_else(|error| unsound(format!("ANS to question {result}: {error}")))?;
    question.settle(state)?;
    let mut edits = vec![question.edit(seq)?];
    let mut asked = Receipt::load(authority, question.record.receipt())?;
    if matches!(asked.head.state(), RoomReceiptState::Pending { .. }) {
        asked
            .head
            .settle(RoomReceiptState::Acked {
                source: publication.source().clone(),
            })
            .or_else(|error| unsound(error.to_string()))?;
        edits.push(asked.edit(seq)?);
    }
    let route = RoomRoute {
        destination: question.record.requester().clone(),
        context: question.record.context().cloned(),
    };
    edits.push(open(seq, publication, 0, route, expires)?);
    Ok(edits)
}

/// Settle every receipt an ACK or NAK names, and decline a live ASK's pending
/// result on NAK even after its receipt retired.
fn controlled(
    authority: &impl Authority,
    seq: Seq,
    controller: &RoomDestination,
    control: &RoomReceiptControl,
) -> Result<Vec<Edit>, Refusal> {
    let mut edits = Vec::new();
    for key in control.keys() {
        let (publication, mut question) = match key {
            RoomDeliveryKey::Route(receipt) => (receipt.publication.clone(), None),
            RoomDeliveryKey::Result { result } => {
                let question = Question::load(authority, result)?;
                (question.record.question().clone(), Some(question))
            }
        };
        let Some(body) = authority.conversation(&publication.to_string())? else {
            return unsound(format!(
                "room delivery key names unadmitted publication {publication}"
            ));
        };
        let admitted: RoomPublication = wire::decode(&body)?;
        let Some(receipt) = admitted.receipt_for(key) else {
            return unsound(format!(
                "room delivery key is not a public key of publication {publication}"
            ));
        };
        let mut receipt = Receipt::load(authority, &receipt)?;
        if &receipt.record.route().destination != controller {
            return unsound(format!(
                "room receipt {} is not addressed to this controller",
                receipt.key
            ));
        }
        let before = receipt.head.state().clone();
        let settled = receipt.head.settle(control.receipt_state());
        let declined = question.as_ref().and_then(|question| {
            matches!(question.head.state(), QuestionResultState::Pending {})
                .then(|| question.record.control_result(control))
                .flatten()
        });
        match settled {
            Ok(()) if receipt.head.state() != &before => edits.push(receipt.edit(seq)?),
            _ if declined.is_some() => {}
            Ok(()) => {
                return unsound(format!(
                    "room receipt {} already carries this disposition",
                    receipt.key
                ));
            }
            Err(error) => return unsound(format!("room receipt {}: {error}", receipt.key)),
        }
        if let (Some(question), Some(state)) = (question.as_mut(), declined) {
            question.settle(state)?;
            edits.push(question.edit(seq)?);
        }
    }
    Ok(edits)
}

/// An explicit decline by the respondent or cancellation by the requester,
/// either of which the operator may override.
fn closed(
    authority: &impl Authority,
    seq: Seq,
    actor: &Actor,
    body: &RoomQuestionClosed,
) -> Result<Vec<Edit>, Refusal> {
    let mut question = Question::load(authority, &body.result)?;
    let source = RoomSource::Journal { fact: code(seq) };
    let (party, receipt_state) = match body.closure {
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
    let mut asked = Receipt::load(authority, question.record.receipt())?;
    if matches!(asked.head.state(), RoomReceiptState::Pending { .. }) {
        asked
            .head
            .settle(receipt_state)
            .or_else(|error| unsound(error.to_string()))?;
        edits.push(asked.edit(seq)?);
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
    unavailable: bool,
) -> Result<Vec<Edit>, Refusal> {
    require_resident(actor, "a room receipt lapse")?;
    let mut receipt = Receipt::load(authority, receipt)?;
    let next = match receipt.head.state() {
        RoomReceiptState::Pending { .. } if unavailable => RoomReceiptState::Unavailable { at },
        RoomReceiptState::Pending { expires } if at >= *expires => RoomReceiptState::Expired { at },
        RoomReceiptState::Pending { .. } => {
            return unsound(format!("room receipt {} has not expired", receipt.key));
        }
        _ => return unsound(format!("room receipt {} is not pending", receipt.key)),
    };
    receipt
        .head
        .settle(next)
        .or_else(|error| unsound(error.to_string()))?;
    Ok(vec![receipt.edit(seq)?])
}

fn recovered(
    authority: &impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    body: &RoomReceiptRecovered,
) -> Result<Vec<Edit>, Refusal> {
    require_operator(actor, "room receipt recovery")?;
    let mut receipt = Receipt::load(authority, &body.receipt)?;
    if body.expires <= at {
        return unsound(format!(
            "recovered room receipt {} must expire after its recovery",
            receipt.key
        ));
    }
    receipt
        .head
        .recover(body.expires)
        .or_else(|error| unsound(format!("room receipt {}: {error}", receipt.key)))?;
    Ok(vec![receipt.edit(seq)?])
}

/// Open one pending receipt for route `index` of `publication`.
fn open(
    seq: Seq,
    publication: &RoomPublication,
    index: usize,
    route: RoomRoute,
    expires: Instant,
) -> Result<Edit, Refusal> {
    let receipt = receipt_ref(publication, index);
    Receipt {
        key: receipt_key(&receipt)?,
        record: RoomReceiptRecord::new(receipt.clone(), route, publication.source().clone()),
        head: RoomReceiptHead::pending(receipt, expires),
    }
    .edit(seq)
}

fn receipt_ref(publication: &RoomPublication, index: usize) -> RoomReceiptRef {
    RoomReceiptRef {
        publication: publication.publication().clone(),
        route: SequenceCode::new(index as u64),
    }
}

/// One decoded receipt head and the immutable record behind it.
struct Receipt {
    key: String,
    record: RoomReceiptRecord,
    head: RoomReceiptHead,
}

impl Receipt {
    fn load(authority: &impl Authority, receipt: &RoomReceiptRef) -> Result<Self, Refusal> {
        let key = receipt_key(receipt)?;
        let Some(row) = authority.room_receipt(&key)? else {
            return unsound(format!("room receipt {key} does not exist"));
        };
        Ok(Self {
            record: wire::decode(&row.record)?,
            head: wire::decode(&row.head)?,
            key,
        })
    }

    fn edit(&self, seq: Seq) -> Result<Edit, Refusal> {
        Ok(Edit::Receipt {
            key: self.key.clone(),
            row: ReceiptRow {
                destination: destination_key(&self.record.route().destination)?,
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
        let Some(row) = authority.question_result(&result.to_string())? else {
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

fn require_lane(actor: &Actor, lane: wire::CanonicalUuid, what: &str) -> Result<(), Refusal> {
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
