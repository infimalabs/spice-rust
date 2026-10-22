//! Retained fact, receipt, and head shapes whose last decoder retired with the
//! release named beside each one.
//!
//! Shapes with an exact current spelling cross in place; shapes without one
//! refuse the crossing, which leaves the predecessor file untouched.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

mod vendor_holds;

/// Last release whose build recorded claimant notices on task edits.
const TASK_EDIT_NOTICE_RELEASE: &str = "a3cebe157";
/// Release from which lane fast facts spell both flags; earlier absence meant false.
const LANE_FAST_FLAGS_RELEASE: &str = "e904cfe7b";
/// Release from which every claim win carries its claim-time cell.
const CLAIM_CELL_RELEASE: &str = "ea3fe662a";
/// Release from which every publication names its release-note digest.
const RELEASE_NOTES_RELEASE: &str = "987f0d60b";
/// Release from which steering heads spell their mint instant in microseconds.
const STEERING_MICROS_RELEASE: &str = "d811711a3";
/// Release from which vendor-failure holds carry their exact refused input.
const VENDOR_FAILURE_INPUT_RELEASE: &str = "3903331e5";
/// Release from which lane session declarations name their tape generation.
const SESSION_SOURCE_RELEASE: &str = "a4abe071f";
/// Release from which resumable sessions carry exact launch identity.
const SESSION_LAUNCH_RELEASE: &str = "9f2487d5a";
/// Release from which steering transcript evidence names its tape generation.
const STEERING_SOURCE_RELEASE: &str = "cb609bd56";
/// Release from which every claim, carry, completion, and review names its proven
/// source boundary.
const LIFECYCLE_SOURCE_RELEASE: &str = "b18f556c3";
/// Retained envelopes rewritten in one bounded crossing step.
const REWRITE_PAGE_ROWS: i64 = 256;
/// Release from which peer-message steering heads retain their frozen room.
const STEERING_ROOM_HEAD_RELEASE: &str = "400b421de";
/// Release from which room packet records name their author occupant and the
/// directives their directed routes opened.
const ROOM_PACKET_AUTHOR_RELEASE: &str = "98a3243fa";
/// Release that recorded room conversations with per-route room receipts
/// before their lane deliveries bound to native steering directives.
const ROOM_CONVERSATION_RELEASE: &str = "d4f4dba9d";
/// Every fact kind that release's room conversation authority wrote.
const ROOM_CONVERSATION_KINDS: [&str; 9] = [
    "room::conversed",
    "room::posted",
    "room::receipted",
    "room::disposed",
    "room::closed",
    "room::timedout",
    "room::lapsed",
    "room::unavailable",
    "room::recovered",
];
/// Last release whose build admitted working groups holding more than
/// [`wire::rooms::WORKING_GROUP_DESIGN_LIMIT`] designs.
const GROUP_DESIGN_LIMIT_RELEASE: &str = "577f4f602";
/// Last release whose build admitted named working groups and removed a
/// group as an object rather than when its last design left.
const UNNAMED_GROUP_RELEASE: &str = "370793eca";
/// Last release whose build stored each working group as its own head
/// through `group::put`, before one partition held them all.
const GROUP_PARTITION_RELEASE: &str = "45eb87f84";
/// Retired per-group fact spellings.
const GROUP_STORED_KIND: &str = "group::stored";
const GROUP_REMOVED_KIND: &str = "group::removed";

/// Refuse every retained shape without a current spelling before mutation, and
/// derive every retained vendor-failure hold's refused input from its history.
pub(super) fn refuse(transaction: &Transaction<'_>) -> Result<vendor_holds::Derived, OpenError> {
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.cell') IS NULL ORDER BY seq",
        [wire::FactKind::ClaimWon.as_str()],
        CLAIM_CELL_RELEASE,
        "the claim win omits its claim-time cell",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.notes') = 'null' ORDER BY seq",
        [wire::FactKind::ReleasePublished.as_str()],
        RELEASE_NOTES_RELEASE,
        "the publication has no release-note digest",
    )?;
    refuse_first(
        transaction,
        "command_receipts",
        "SELECT command_id FROM command_receipts WHERE command_kind = ?1 \
         AND json_type(CAST(request AS TEXT), '$.body.notes') = 'null' ORDER BY command_id",
        [wire::CommandKind::ReleasePublish.as_str()],
        RELEASE_NOTES_RELEASE,
        "the publication request has no release-note digest",
    )?;
    refuse_first(
        transaction,
        "steering_heads",
        "SELECT directive FROM steering_heads \
         WHERE json_type(CAST(body AS TEXT), '$.minted_millis') IS NOT NULL ORDER BY directive",
        (),
        STEERING_MICROS_RELEASE,
        "the steering head spells its mint instant in milliseconds",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.source') IS NULL ORDER BY seq",
        [wire::FactKind::LaneSessionDeclared.as_str()],
        SESSION_SOURCE_RELEASE,
        "the lane session declaration omits its tape generation",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND (json_type(CAST(envelope AS TEXT), '$.fact.model') IS NULL \
              OR json_type(CAST(envelope AS TEXT), '$.fact.effort') IS NULL) ORDER BY seq",
        [wire::FactKind::LaneSessionDeclared.as_str()],
        SESSION_LAUNCH_RELEASE,
        "the lane session declaration omits its launch identity",
    )?;
    refuse_first(
        transaction,
        "residency_head",
        "SELECT CAST(singleton AS TEXT) FROM residency_head, \
         json_each(CAST(body AS TEXT), '$.lanes') AS lane \
         WHERE json_type(lane.value, '$.session') = 'object' \
         AND (json_type(lane.value, '$.session.model') IS NULL \
              OR json_type(lane.value, '$.session.effort') IS NULL) ORDER BY singleton",
        (),
        SESSION_LAUNCH_RELEASE,
        "the resumable lane session omits its launch identity",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_array_length(CAST(envelope AS TEXT), '$.fact.group.anchors') > ?2 ORDER BY seq",
        params![
            GROUP_STORED_KIND,
            u32::try_from(wire::rooms::WORKING_GROUP_DESIGN_LIMIT)
                .expect("the design cap is small")
        ],
        GROUP_DESIGN_LIMIT_RELEASE,
        "the working group holds more designs than a group admits",
    )?;
    refuse_retired_groups(transaction)?;
    refuse_unbound_provenance(transaction)?;
    vendor_holds::derive(transaction)
}

/// Refuse working groups an earlier release stored by name or as their own
/// heads, each naming the last release that wrote them; a named group is
/// refused ahead of the partition crossing that also covers it.
fn refuse_retired_groups(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.group.name') IS NOT NULL ORDER BY seq",
        [GROUP_STORED_KIND],
        UNNAMED_GROUP_RELEASE,
        "the working group carries an operator-given name",
    )?;
    refuse_first(
        transaction,
        "command_receipts",
        "SELECT command_id FROM command_receipts WHERE (command_kind = ?1 \
         AND json_type(CAST(request AS TEXT), '$.body.group.name') IS NOT NULL) \
         OR command_kind = 'group::remove' ORDER BY command_id",
        ["group::put"],
        UNNAMED_GROUP_RELEASE,
        "the working-group request names its group or removes it as an object",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind IN (?1, ?2) ORDER BY seq",
        [GROUP_STORED_KIND, GROUP_REMOVED_KIND],
        GROUP_PARTITION_RELEASE,
        "the working group is stored as its own head rather than in the partition",
    )?;
    refuse_first(
        transaction,
        "command_receipts",
        "SELECT command_id FROM command_receipts WHERE command_kind = 'group::put' \
         ORDER BY command_id",
        (),
        GROUP_PARTITION_RELEASE,
        "the working-group request replaces one group rather than moving a design",
    )
}

/// Refuse retained facts that omit what binds them to their tape generation,
/// proven source boundary, or packet author and routes.
fn refuse_unbound_provenance(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    for kind in [
        wire::FactKind::SteerAcked,
        wire::FactKind::SteerDeclined,
        wire::FactKind::SteerMissed,
        wire::FactKind::SteerMessaged,
        wire::FactKind::SteerWithheld,
    ] {
        refuse_first(
            transaction,
            "fact_envelopes",
            "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
             AND json_type(CAST(envelope AS TEXT), '$.fact.locus.source') IS NULL ORDER BY seq",
            [kind.as_str()],
            STEERING_SOURCE_RELEASE,
            "the steering transcript locus omits its tape generation",
        )?;
    }
    for kind in [
        wire::FactKind::ClaimWon,
        wire::FactKind::ClaimCarried,
        wire::FactKind::TaskDone,
        wire::FactKind::TaskReviewed,
    ] {
        refuse_first(
            transaction,
            "fact_envelopes",
            "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
             AND json_type(CAST(envelope AS TEXT), '$.fact.source.proven') IS NULL ORDER BY seq",
            [kind.as_str()],
            LIFECYCLE_SOURCE_RELEASE,
            "the lifecycle fact has no proven source boundary",
        )?;
    }
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND (json_type(CAST(envelope AS TEXT), '$.fact.author') IS NULL \
              OR json_type(CAST(envelope AS TEXT), '$.fact.directed') IS NULL) ORDER BY seq",
        [wire::FactKind::RoomMessaged.as_str()],
        ROOM_PACKET_AUTHOR_RELEASE,
        "the room packet record omits its author occupant and directed routes",
    )?;
    refuse_retired_conversations(transaction)?;
    Ok(())
}

/// Conversation facts may already carry the current delivery contract even
/// while their journal still awaits another schema crossing. Refuse only
/// records the current strict codec cannot admit; never infer their shape from
/// their kind or the journal's predecessor version.
fn refuse_retired_conversations(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let mut statement = transaction
        .prepare("SELECT seq, envelope FROM fact_envelopes WHERE kind = ?1 ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    for kind in ROOM_CONVERSATION_KINDS {
        let rows = statement
            .query_map([kind], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?;
        for row in rows {
            let (seq, bytes) = row.map_err(OpenError::Sqlite)?;
            let envelope =
                wire::decode::<wire::Envelope<wire::AnyFact>>(&bytes).map_err(|error| {
                    retained(
                        "fact_envelopes",
                        seq.to_string(),
                        ROOM_CONVERSATION_RELEASE,
                        format!(
                            "the room conversation record has no current delivery spelling: {error}"
                        ),
                    )
                })?;
            if !envelope.v.is_current()
                || envelope.fact.kind().as_str() != kind
                || Some(envelope.seq.get()) != u64::try_from(seq).ok()
            {
                return Err(retained(
                    "fact_envelopes",
                    seq.to_string(),
                    ROOM_CONVERSATION_RELEASE,
                    "the room conversation record disagrees with its indexed identity".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

/// Rewrite every retained shape that has one exact current spelling.
pub(super) fn cross(
    transaction: &Transaction<'_>,
    vendor_holds: vendor_holds::Derived,
) -> Result<(), OpenError> {
    rewrite_facts(
        transaction,
        wire::FactKind::TaskEdited,
        "json_type(CAST(envelope AS TEXT), '$.fact.notice') IS NOT NULL",
        TASK_EDIT_NOTICE_RELEASE,
        |_, fact| match fact.remove("notice") {
            Some(Value::Object(_)) => Ok(()),
            other => Err(format!("notice {other:?} is not a claimant notice object")),
        },
    )?;
    rewrite_facts(
        transaction,
        wire::FactKind::LaneFastRetuned,
        "json_type(CAST(envelope AS TEXT), '$.fact.from') IS NULL \
         OR json_type(CAST(envelope AS TEXT), '$.fact.fast_mode') IS NULL",
        LANE_FAST_FLAGS_RELEASE,
        |_, fact| {
            for flag in ["from", "fast_mode"] {
                fact.entry(flag).or_insert(Value::Bool(false));
            }
            Ok(())
        },
    )?;
    vendor_holds::cross(transaction, vendor_holds)
}

/// Project the exact room anchor frozen by each retained peer message into its
/// steering head. Schema 39 stored the anchor only on the immutable creating
/// fact; crossing copies that evidence once so current inbox reads use one
/// bounded authority row.
pub(super) fn cross_steering_rooms(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT heads.directive, heads.body, facts.envelope \
                 FROM steering_heads AS heads \
                 JOIN fact_envelopes AS facts ON facts.seq = heads.created_seq \
                 WHERE facts.kind = ?1 \
                 AND json_type(CAST(heads.body AS TEXT), '$.room') IS NULL \
                 AND json_type(CAST(facts.envelope AS TEXT), '$.fact.room') IS NOT NULL \
                 ORDER BY heads.directive",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([wire::FactKind::SteerMessaged.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (directive, head_bytes, fact_bytes) in rows {
        let refuse = |detail: String| {
            retained(
                "steering_heads",
                directive.clone(),
                STEERING_ROOM_HEAD_RELEASE,
                detail,
            )
        };
        let envelope = wire::decode::<wire::Envelope<wire::AnyFact>>(&fact_bytes)
            .map_err(|error| refuse(format!("the creating fact does not decode: {error}")))?;
        let message = envelope
            .fact
            .read::<wire::body::SteerMessaged>()
            .ok_or_else(|| refuse("the indexed creating fact is not a peer message".to_owned()))?;
        let room = message.room.ok_or_else(|| {
            refuse("the selected peer message carries no frozen room anchor".to_owned())
        })?;
        let mut head: Value = serde_json::from_slice(&head_bytes)
            .map_err(|error| refuse(format!("the steering head does not decode: {error}")))?;
        let object = head
            .as_object_mut()
            .ok_or_else(|| refuse("the steering head is not an object".to_owned()))?;
        object.insert(
            "room".to_owned(),
            serde_json::to_value(room)
                .map_err(|error| refuse(format!("the room anchor cannot encode: {error}")))?,
        );
        let crossed = serde_json::to_vec(&head)
            .map_err(|error| refuse(format!("the crossed head cannot encode: {error}")))?;
        let current = wire::decode::<wire::SteeringHead>(&crossed)
            .map_err(|error| refuse(format!("the crossed head is not current: {error}")))?;
        let bytes = wire::encode(&current)
            .map_err(|error| refuse(format!("the crossed head cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE steering_heads SET body = ?1 WHERE directive = ?2",
                params![bytes, directive],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Refuse naming the first row `sql` selects, if any.
pub(super) fn refuse_first(
    connection: &Connection,
    table: &'static str,
    sql: &str,
    parameters: impl rusqlite::Params,
    release: &'static str,
    detail: &str,
) -> Result<(), OpenError> {
    match connection
        .query_row(sql, parameters, |row| row.get::<_, String>(0))
        .optional()
        .map_err(OpenError::Sqlite)?
    {
        Some(row) => Err(retained(table, row, release, detail.to_owned())),
        None => Ok(()),
    }
}

/// Rewrite each `kind` fact `predicate` selects through `edit` on its seq and body.
///
/// Each envelope keeps its seq, indexed columns, and every other member, and is
/// re-encoded canonically through the current strict DTO. No digest covers an
/// individual envelope, so nothing downstream disagrees with the new bytes.
pub(super) fn rewrite_facts(
    transaction: &Transaction<'_>,
    kind: wire::FactKind,
    predicate: &str,
    release: &'static str,
    edit: impl Fn(i64, &mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), OpenError> {
    let kind = kind.as_str();
    let mut after = 0_i64;
    loop {
        let rows = {
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT seq, envelope FROM fact_envelopes \
                     WHERE kind = ?1 AND seq > ?2 AND ({predicate}) \
                     ORDER BY seq LIMIT ?3"
                ))
                .map_err(OpenError::Sqlite)?;
            let rows = statement
                .query_map(params![kind, after, REWRITE_PAGE_ROWS], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(OpenError::Sqlite)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(OpenError::Sqlite)?
        };
        let Some((last, _)) = rows.last() else {
            return Ok(());
        };
        after = *last;
        for (seq, bytes) in rows {
            let refuse =
                |detail: String| retained("fact_envelopes", seq.to_string(), release, detail);
            let mut envelope: Value = serde_json::from_slice(&bytes)
                .map_err(|error| refuse(format!("the stored envelope does not decode: {error}")))?;
            let fact = envelope
                .get_mut("fact")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| refuse("the stored envelope carries no fact object".to_owned()))?;
            edit(seq, fact).map_err(refuse)?;
            let current = serde_json::to_vec(&envelope)
                .map_err(|error| refuse(format!("the crossed envelope cannot encode: {error}")))
                .and_then(|crossed| {
                    wire::decode::<wire::Envelope<wire::AnyFact>>(&crossed).map_err(|error| {
                        refuse(format!("the crossed envelope is not current: {error}"))
                    })
                })?;
            let bytes = wire::encode(&current)
                .map_err(|error| refuse(format!("the crossed envelope cannot encode: {error}")))?;
            let changed = transaction
                .execute(
                    "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2 AND kind = ?3",
                    params![bytes, seq, kind],
                )
                .map_err(OpenError::Sqlite)?;
            if changed != 1 {
                return Err(refuse(
                    "the row changed under the exclusive crossing".to_owned(),
                ));
            }
        }
    }
}

/// Why one receipt edit refuses its row: the detail, under the crossing's
/// release unless the edit names the release that wrote the refused shape.
pub(super) struct Refusal {
    release: Option<&'static str>,
    detail: String,
}

impl Refusal {
    /// A refusal naming its own release instead of the crossing's.
    pub(super) const fn under(release: &'static str, detail: String) -> Self {
        Self {
            release: Some(release),
            detail,
        }
    }
}

impl From<String> for Refusal {
    fn from(detail: String) -> Self {
        Self {
            release: None,
            detail,
        }
    }
}

/// Rewrite each retained `command` receipt `predicate` selects through `edit`
/// on its request and response bytes; `None` for either keeps the stored
/// bytes, and no predicate selects every receipt of the command.
pub(super) fn rewrite_receipts(
    transaction: &Transaction<'_>,
    command: wire::CommandKind,
    predicate: Option<&str>,
    release: &'static str,
    edit: impl Fn(&[u8], &[u8]) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), Refusal>,
) -> Result<(), OpenError> {
    let sql = format!(
        "SELECT command_id, request, response FROM command_receipts \
         WHERE command_kind = ?1 AND ({}) ORDER BY command_id",
        predicate.unwrap_or("1")
    );
    let receipts = {
        let mut statement = transaction.prepare(&sql).map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map(params![command.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (command_id, request, response) in receipts {
        let refuse = |refusal: Refusal| {
            retained(
                "command_receipts",
                command_id.clone(),
                refusal.release.unwrap_or(release),
                refusal.detail,
            )
        };
        let (request, response) = edit(&request, &response).map_err(refuse)?;
        if request.is_none() && response.is_none() {
            continue;
        }
        let changed = transaction
            .execute(
                "UPDATE command_receipts \
                 SET request = coalesce(?1, request), response = coalesce(?2, response) \
                 WHERE command_id = ?3 AND command_kind = ?4",
                params![request, response, command_id, command.as_str()],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing"
                    .to_owned()
                    .into(),
            ));
        }
    }
    Ok(())
}

/// The retained request with its body object edited through `edit`, re-parsed
/// through the current strict `wire::Command<T>` DTO and re-encoded
/// canonically; `None` when `edit` answers that it changed nothing.
pub(super) fn edit_request<T, E>(
    bytes: &[u8],
    edit: impl FnOnce(&mut Map<String, Value>) -> Result<bool, E>,
) -> Result<Option<Vec<u8>>, Refusal>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
    E: Into<Refusal>,
{
    let mut request: Value = wire::decode(bytes)
        .map_err(|error| format!("the stored request does not decode: {error}"))?;
    let body = request
        .get_mut("body")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "the stored request has no body object".to_owned())?;
    if !edit(body).map_err(Into::into)? {
        return Ok(None);
    }
    let request: wire::Command<T> = serde_json::from_value(request)
        .map_err(|error| format!("the crossed request is not current: {error}"))?;
    Ok(request
        .canonical_json()
        .map(String::into_bytes)
        .map(Some)
        .map_err(|error| format!("the crossed request cannot encode: {error}"))?)
}

/// The retained response with its reply object edited through `edit` and
/// re-checked against `command`'s answer binding; `None` for a refusal or when
/// `edit` answers that it changed nothing.
pub(super) fn edit_reply<E: Into<Refusal>>(
    command: wire::CommandKind,
    bytes: &[u8],
    edit: impl FnOnce(&mut Map<String, Value>) -> Result<bool, E>,
) -> Result<Option<Vec<u8>>, Refusal> {
    let mut response: wire::Response<Value> = wire::decode(bytes)
        .map_err(|error| format!("the stored response does not decode: {error}"))?;
    let wire::Outcome::Reply(reply) = &mut response.outcome else {
        return Ok(None);
    };
    let answer = reply
        .body
        .as_object_mut()
        .ok_or_else(|| "the stored answer is not a JSON object".to_owned())?;
    if !edit(answer).map_err(Into::into)? {
        return Ok(None);
    }
    wire::encode(&reply.body)
        .and_then(|body| command.binding().answer.decode(&body))
        .map_err(|error| format!("the crossed answer is not current: {error}"))?;
    Ok(wire::encode(&response)
        .map(Some)
        .map_err(|error| format!("the crossed response cannot encode: {error}"))?)
}

/// The retained-row refusal every migration crossing builds its refusals from.
pub(crate) fn retained(
    table: &'static str,
    row: String,
    release: &'static str,
    detail: String,
) -> OpenError {
    MigrationError::RetainedRow {
        table,
        row,
        release,
        detail,
    }
    .into()
}
