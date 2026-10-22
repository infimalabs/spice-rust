use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::SCHEMA_VERSION;

mod authorization_heads;
mod awareness;
mod contribution_charges;
mod contribution_items;
mod creation_contexts;
mod decisions;
mod design_asks;
mod design_invitations;
mod designs;
mod fact_bounds;
mod leave_receipts;
mod maturity;
mod oops_design;
mod oops_generation;
mod pitch;
mod preparation_edges;
mod provenance;
pub(crate) mod residual;
mod retained_notes;
mod review_status;
mod room_questions;
mod room_rules;
mod settings_receipts;
mod steering_rooms;
mod task_evidence;
mod task_markers;
mod task_standing;
mod task_tags;
mod taste_holds;
mod team_formations;
mod workflows;
mod working_groups;

#[cfg(test)]
pub(crate) use fact_bounds::bounds_sql;
pub(crate) use fact_bounds::{
    FACT_ENVELOPE_INSERT_TRIGGER, FACT_ENVELOPE_UPDATE_TRIGGER, insert_sql, update_sql,
};
const FINALIZED: &str = "finalized";
const PREPARED: &str = "prepared";

/// Oldest schema admitted through the one current writable-open crossing;
/// every schema from it up to [`SCHEMA_VERSION`] crosses.
///
/// Task fields already have their current semantics. The crossing first proves,
/// before any mutation, that no retained task fact or head omits the creation
/// context schema 35 was the last to default. It then writes the explicit
/// subscription table a residency head may still omit, replaces retired fact
/// kinds with inert current-schema evidence, drops retired plan-advance
/// guidance from task completions and their receipts, derives retained vendor
/// holds' exact input from preceding authority or refuses unproved input,
/// incomplete launch identity, and source-less session or steering authority,
/// crosses or refuses the remaining retired fact, receipt,
/// and head shapes, refuses room packet records that omit their author and
/// directed routes, room coordination records written before the journal
/// bound them, room conversation records without the current native steering
/// delivery shape, working groups holding more designs than a group admits,
/// named working groups or group removals from before a group lived only
/// while it held designs, and working groups stored as their own heads rather
/// than in the partition, projects frozen peer-message rooms from their
/// creating facts into steering heads, moves each open Design onto its one
/// open phase and drops its pending outputs' ordinary dependency on it,
/// restates every live team's formation instant from immutable topology facts,
/// binds retained Design notices to their exact statement while retiring
/// superseded selection provenance, records each retained task-backed notice's
/// claim-phase classification and bounded excerpt and elides the excerpt of a
/// retained conversation notice that kept no prose, and creates the room-authority projection,
/// room catch-up cursors and horizons, room coordination heads, room
/// conversation heads, operator items, relevance
/// shadow records, the working-group partition, the empty principal head
/// index, and the per-Design notice and bounded maturity projections a
/// predecessor lacks, replacing its per-route room receipts and per-group
/// heads. Finally it crosses every retained settings command answer from the
/// retired operator field to the current principal directory, each entry
/// carrying its principal's code, makes retained working-group activity
/// stance explicit, lifts task priority, taste-review standing, document slugs,
/// and taste-proposal evidence from retired statement tags into typed fields,
/// then discards every remaining tag before the schema identity advances. Each
/// taste hold takes the matches and recheck count its retired audit notes
/// recorded. Last, every retained task note that is not a taste-review audit
/// joins its task's room projection at its own position, folded exactly as
/// verification replays it. The final crossing replaces retained derived
/// preparation authorization with ordinary dependency edges, gates every
/// pending output of an open Design, and closes every pending branch whose
/// terminal or absent controller cannot release it. Before any current decoder
/// sees a predecessor statement, the retired authorization field is lifted into
/// transaction-local selection evidence, and each task whose creation context
/// holds a retired unavailable reason on its creating fact or statement head
/// takes the reason its creating actor proves on both, refusing a lane creator,
/// a missing creating fact, a copy already contradicting that reason, and any
/// boundary on an unregistered transcript stream. Last, every steering head
/// takes its body's room as a typed indexed column, every retained task review
/// takes the workflow status its verdict left, and every retained task Git
/// boundary and review correction enters the release-note projection. Before
/// any of that, the question-result protocol and required question-result
/// evidence retire into inert historical authority, every retained
/// contribution label with its receipt crosses to its charge, every
/// retained item-bearing contribution takes its item's text identity,
/// every retained room coordination record, direction head, and coordination
/// receipt loses its retired automated-resume rule, every retained
/// single-ask Design invitation takes its one-element ask list, and every
/// retained leave answer loses its retired repeat-leave flag.
const OLDEST_PREDECESSOR_SCHEMA: u32 = 33;

/// Whether `version` is a predecessor this release crosses forward.
fn is_predecessor(version: u32) -> bool {
    (OLDEST_PREDECESSOR_SCHEMA..SCHEMA_VERSION).contains(&version)
}

/// Release that migrated task facts and heads to explicit creation context.
const CREATION_CONTEXT_RELEASE: &str = "6ad57c713";
/// Release that added the residency subscription table without a schema step.
const RESIDENCY_SUBSCRIPTIONS_RELEASE: &str = "4047a2a4f";
/// Release whose formation instants follow the one teamed predicate, so a lone
/// member under another team's identity is formed.
const TEAM_FORMATIONS_RELEASE: &str = "schema-60";
/// First schema whose baseline persists formation instants under the one
/// teamed predicate.
const TEAM_FORMATIONS_SCHEMA: u32 = 60;
/// First schema whose baseline carries the principal head index.
pub(crate) const PRINCIPAL_HEADS_SCHEMA: u32 = 59;
/// The principal head index. Admission facts first exist at
/// [`PRINCIPAL_HEADS_SCHEMA`], so both initialization and the crossing create
/// it empty.
const PRINCIPAL_HEADS_SQL: &str = "CREATE TABLE principal_heads (
    principal TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;";
/// First schema whose baseline carries the room-authority projection.
pub(crate) const ROOM_AUTHORITY_SCHEMA: u32 = 36;
/// First schema whose baseline carries per-lane room catch-up cursors.
pub(crate) const ROOM_CURSOR_SCHEMA: u32 = 41;
/// First schema whose baseline carries room coordination heads.
pub(crate) const ROOM_COORDINATION_SCHEMA: u32 = 44;
/// First schema whose baseline carries per-lane room catch-up horizons.
pub(crate) const ROOM_HORIZON_SCHEMA: u32 = 45;
/// First schema whose baseline carries room conversation publications and
/// result heads.
pub(crate) const ROOM_CONVERSATION_SCHEMA: u32 = 46;
/// First schema whose baseline carries operator items in place of per-route
/// room receipts.
pub(crate) const ROOM_ITEM_SCHEMA: u32 = 47;
/// First schema whose baseline carries room relevance shadow observations.
pub(crate) const ROOM_RELEVANCE_SCHEMA: u32 = 49;
/// First schema whose baseline carries the working-group partition in place
/// of per-group heads.
pub(crate) const GROUP_PARTITION_SCHEMA: u32 = 53;
/// Last schema whose current decision shape predates output verdicts.
const WHOLE_CANDIDATE_DECISION_SCHEMA: u32 = 56;
/// First schema whose baseline carries bounded Design maturity and notices.
pub(crate) const DESIGN_MATURITY_SCHEMA: u32 = 63;
/// First schema whose Design pass proposals and output decisions carry their
/// current explicit shape.
const DESIGN_PASS_SCHEMA: u32 = 73;
/// First schema whose room projection indexes retained task notes.
pub(crate) const RETAINED_NOTES_SCHEMA: u32 = 76;
/// First schema whose baseline carries the repository OOPS-generation head.
pub(crate) const OOPS_GENERATION_SCHEMA: u32 = 77;
/// First schema whose baseline indexes each steering head's room, and the only
/// retained schema whose executable authority still carries room questions.
pub(crate) const STEERING_ROOM_SCHEMA: u32 = 86;
/// First schema whose baseline carries the release-note projection.
pub(crate) const RELEASE_NOTES_SCHEMA: u32 = 88;
/// Each room projection beside the first schema whose baseline carries it, in
/// creation order.
const ROOM_SCHEMA_STEPS: [(u32, &str); 10] = [
    (ROOM_AUTHORITY_SCHEMA, crate::rooms::SCHEMA_SQL),
    (ROOM_CURSOR_SCHEMA, crate::rooms::CURSOR_SCHEMA_SQL),
    (
        ROOM_COORDINATION_SCHEMA,
        crate::rooms::COORDINATION_SCHEMA_SQL,
    ),
    (ROOM_HORIZON_SCHEMA, crate::rooms::HORIZON_SCHEMA_SQL),
    (
        ROOM_CONVERSATION_SCHEMA,
        crate::rooms::CONVERSATION_SCHEMA_SQL,
    ),
    (ROOM_ITEM_SCHEMA, crate::rooms::CONVERSATION_ITEM_SCHEMA_SQL),
    (ROOM_RELEVANCE_SCHEMA, crate::rooms::RELEVANCE_SCHEMA_SQL),
    (GROUP_PARTITION_SCHEMA, crate::rooms::GROUP_SCHEMA_SQL),
    (DESIGN_MATURITY_SCHEMA, crate::rooms::MATURITY_SCHEMA_SQL),
    (
        OOPS_GENERATION_SCHEMA,
        crate::rooms::OOPS_GENERATION_SCHEMA_SQL,
    ),
];
const RETIRED_GATE_FILED_KIND: &str = "gate::filed";
const RETIRED_GATE_FILED_RELEASE: &str = "595da8d81";
/// Last release whose wire decoded task completion guidance.
const RETIRED_TASK_GUIDANCE_RELEASE: &str = "979b72704";
/// The sole guidance spelling that release could decode.
const RETIRED_TASK_GUIDANCE: &str = concat!(
    "phase ownership: this advance released your claim; only the held plan phase ",
    "was completed. Run spice task next for another assignment; it may select any ",
    "eligible work and does not guarantee this task's next phase."
);

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredGateFiled {
    kind: RetiredGateFiledKind,
    report: wire::Digest,
    findings: u32,
    created: u32,
    reused: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
enum RetiredGateFiledKind {
    #[serde(rename = "gate::filed")]
    GateFiled,
}

struct RetiredGateFiledRow {
    seq: i64,
    wire_version: i64,
    instant: String,
    actor: String,
    origin: String,
    cause: Option<i64>,
    envelope: Vec<u8>,
}

/// Admit exact-current authority, cross the predecessor once, or initialize
/// one truly empty database.
///
/// Existing authority is classified without a transaction so a retired file
/// refuses before SQLite can create a rollback journal or change its persistent
/// journal mode. Empty initialization and the predecessor crossing take the
/// exclusive transaction, then recheck what they observed to close the race
/// with another opener.
pub(crate) fn admit(connection: &mut Connection) -> Result<Option<Migration>, OpenError> {
    let trusted_schema: bool = connection
        .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
        .map_err(OpenError::Sqlite)?;
    if trusted_schema {
        return Err(OpenError::TrustedSchemaEnabled);
    }

    if table_exists(connection, "journal_schema")? {
        let version = admit_readable_state(connection, stored_state(connection)?)?;
        if !is_predecessor(version) {
            admit_version(version)?;
            return Ok(None);
        }
        return cross_predecessor(connection);
    }
    let object_count = authority_object_count(connection)?;
    if object_count != 0 {
        return Err(MigrationError::UnversionedAuthority { object_count }.into());
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(OpenError::Sqlite)?;
    if table_exists(&transaction, "journal_schema")? {
        admit_state(&transaction, stored_state(&transaction)?)?;
        transaction.commit().map_err(OpenError::Sqlite)?;
        return Ok(None);
    }
    let object_count = authority_object_count(&transaction)?;
    if object_count != 0 {
        return Err(MigrationError::UnversionedAuthority { object_count }.into());
    }

    transaction
        .execute_batch(BASELINE_SQL)
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO journal_schema(singleton, version, state) VALUES (1, ?1, ?2)",
            params![SCHEMA_VERSION, FINALIZED],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(OpenError::Sqlite)?;
    create_room_schemas(&transaction, 0)?;
    create_principal_heads(&transaction, 0)?;
    crate::historical_receipts::create_schema(&transaction)?;
    crate::historical_receipts::reserve_identities(&transaction)?;
    transaction
        .execute_batch(&fact_bounds::bounds_sql())
        .map_err(OpenError::Sqlite)?;
    transaction.commit().map_err(OpenError::Sqlite)?;
    Ok(Some(Migration {
        from: 0,
        to: SCHEMA_VERSION,
    }))
}

/// Current-schema initialization or predecessor crossing completed by this open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Migration {
    from: u32,
    to: u32,
}

impl Migration {
    /// Schema identity before this open: `0` for initialization, otherwise
    /// the crossed predecessor.
    #[must_use]
    pub const fn from(self) -> u32 {
        self.from
    }

    /// Current schema this open reached.
    #[must_use]
    pub const fn to(self) -> u32 {
        self.to
    }
}

pub(crate) fn finalized_version(connection: &Connection) -> Result<u32, OpenError> {
    admit_state(connection, stored_state(connection)?)
}

fn stored_state(connection: &Connection) -> Result<SchemaState, OpenError> {
    let count: u32 = connection
        .query_row("SELECT count(*) FROM journal_schema", [], |row| row.get(0))
        .map_err(OpenError::Sqlite)?;
    if count != 1 {
        return Err(MigrationError::SchemaRowCount { count }.into());
    }

    let state = connection
        .query_row(
            "SELECT version, state, from_version, to_version FROM journal_schema WHERE singleton = 1",
            [],
            |row| {
                Ok(SchemaState {
                    version: row.get(0)?,
                    state: row.get(1)?,
                    from_version: row.get(2)?,
                    to_version: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(OpenError::Sqlite)?
        .ok_or(MigrationError::MissingSingleton)?;
    Ok(state)
}

/// Reads an existing schema identity without performing baseline or migration.
pub(crate) fn inspect_version(connection: &Connection) -> Result<u32, OpenError> {
    if !table_exists(connection, "journal_schema")? {
        let object_count = authority_object_count(connection)?;
        return Err(if object_count == 0 {
            MigrationError::MissingSchema
        } else {
            MigrationError::UnversionedAuthority { object_count }
        }
        .into());
    }
    finalized_version(connection)
}

/// Observe a finalized stored schema identity without admitting its version.
///
/// Upgrade orchestration needs the exact before/after version even when this
/// build would refuse to read that version's authority. The identity still has
/// to be finalized and agree with SQLite's independent version record.
pub(crate) fn inspect_stored_version(connection: &Connection) -> Result<u32, OpenError> {
    if !table_exists(connection, "journal_schema")? {
        let object_count = authority_object_count(connection)?;
        return Err(if object_count == 0 {
            MigrationError::MissingSchema
        } else {
            MigrationError::UnversionedAuthority { object_count }
        }
        .into());
    }
    finalized_stored_state(connection, stored_state(connection)?)
}

/// Admit a finalized authority at the sole executable schema.
fn admit_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
    admit_version(admit_readable_state(connection, state)?)
}

fn admit_version(version: u32) -> Result<u32, OpenError> {
    if is_predecessor(version) {
        return Err(MigrationError::PendingCrossing { found: version }.into());
    }
    if version != SCHEMA_VERSION {
        return Err(MigrationError::UnknownVersion { found: version }.into());
    }
    Ok(version)
}

fn cross_current_rows(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<taste_holds::Audits, OpenError> {
    creation_contexts::cross(transaction)?;
    workflows::cross(transaction)?;
    oops_design::cross(transaction)?;
    provenance::cross(transaction)?;
    task_tags::cross_heads(transaction)?;
    pitch::cross_heads(transaction)?;
    let taste_audits = taste_holds::Audits::read(transaction)?;
    taste_holds::cross_heads(transaction, &taste_audits)?;
    Ok(taste_audits)
}

/// Remove each retired member a current decoder would refuse, before any
/// later crossing decodes a retained row strictly.
fn strip_retired_members(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    preparation_edges::strip_authorization(transaction)?;
    task_evidence::cross(transaction)?;
    contribution_charges::cross(transaction)?;
    contribution_items::cross(transaction)?;
    room_rules::cross(transaction)?;
    design_asks::cross(transaction)?;
    leave_receipts::cross(transaction)
}

/// Move a finalized predecessor authority to the current schema exactly once.
fn cross_predecessor(connection: &mut Connection) -> Result<Option<Migration>, OpenError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(OpenError::Sqlite)?;
    let version = admit_readable_state(&transaction, stored_state(&transaction)?)?;
    if !is_predecessor(version) {
        admit_version(version)?;
        transaction.commit().map_err(OpenError::Sqlite)?;
        return Ok(None);
    }
    cross_room_questions(&transaction, version)?;
    strip_retired_members(&transaction)?;
    let taste_audits = cross_current_rows(&transaction)?;
    review_status::cross(&transaction)?;
    cross_schema_shape(&transaction, version)?;
    cross_room_successors(&transaction, version)?;
    create_principal_heads(&transaction, version)?;
    working_groups::cross(&transaction)?;
    if version < OOPS_GENERATION_SCHEMA {
        oops_generation::cross(&transaction)?;
    }
    pitch::cross_facts(&transaction)?;
    task_tags::cross_facts(&transaction, &taste_audits)?;
    taste_holds::cross_facts(&transaction, &taste_audits)?;
    settings_receipts::cross(&transaction)?;
    design_invitations::cross(&transaction)?;
    awareness::cross(&transaction)?;
    maturity::cross(&transaction)?;
    cross_retained_notes(&transaction, version)?;
    preparation_edges::cross(&transaction)?;
    if version < RELEASE_NOTES_SCHEMA {
        crate::release_notes::cross(&transaction)?;
    }
    advance_identity(&transaction)?;
    transaction.commit().map_err(OpenError::Sqlite)?;
    Ok(Some(Migration {
        from: version,
        to: SCHEMA_VERSION,
    }))
}

fn cross_schema_shape(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<(), OpenError> {
    if version <= WHOLE_CANDIDATE_DECISION_SCHEMA {
        return cross_shape_predecessor(transaction, version);
    }
    create_room_schemas(transaction, version)?;
    if version < DESIGN_PASS_SCHEMA {
        decisions::cross_design_passes(transaction)?;
        decisions::cross_output_decisions(transaction)?;
    }
    Ok(())
}

fn cross_room_questions(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<(), OpenError> {
    if version == STEERING_ROOM_SCHEMA {
        room_questions::cross(transaction)
    } else {
        room_questions::refuse_retained(transaction)
    }
}

fn cross_room_successors(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<(), OpenError> {
    authorization_heads::cross(transaction, version)?;
    if version < TEAM_FORMATIONS_SCHEMA {
        team_formations::cross(transaction)?;
    }
    Ok(())
}

fn cross_retained_notes(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<(), OpenError> {
    if version < RETAINED_NOTES_SCHEMA {
        retained_notes::cross(transaction)?;
    }
    Ok(())
}

/// Index every settled steering head's room, then name the crossed authority
/// the current schema.
fn advance_identity(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let room_is_typed = transaction
        .query_row(
            "SELECT EXISTS(\
                 SELECT 1 FROM pragma_table_info('steering_heads') WHERE name = 'room'\
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(OpenError::Sqlite)?;
    if !room_is_typed {
        steering_rooms::cross(transaction)?;
    }
    transaction
        .execute(
            "UPDATE journal_schema SET version = ?1 WHERE singleton = 1",
            params![SCHEMA_VERSION],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(OpenError::Sqlite)
}

fn cross_shape_predecessor(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<(), OpenError> {
    refuse_creation_omission(transaction)?;
    let vendor_holds = residual::refuse(transaction)?;
    cross_residency_subscriptions(transaction)?;
    cross_retired_gate_filed(transaction)?;
    cross_retired_task_guidance(transaction)?;
    residual::cross(transaction, vendor_holds)?;
    residual::cross_steering_rooms(transaction)?;
    if version >= ROOM_AUTHORITY_SCHEMA {
        decisions::cross_design_passes(transaction)?;
        decisions::cross_two_step(transaction)?;
    }
    designs::cross(transaction)?;
    if (ROOM_CONVERSATION_SCHEMA..ROOM_ITEM_SCHEMA).contains(&version) {
        transaction
            .execute_batch("DROP TABLE room_receipts;")
            .map_err(OpenError::Sqlite)?;
    }
    create_room_schemas(transaction, version)
}

/// Create each room projection an authority at `version` lacks; version zero
/// lacks them all.
fn create_room_schemas(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<(), OpenError> {
    for (since, sql) in ROOM_SCHEMA_STEPS {
        if version < since {
            if since == DESIGN_MATURITY_SCHEMA {
                transaction
                    .execute_batch(
                        "DROP TABLE IF EXISTS design_maturity;\
                         DROP TABLE IF EXISTS design_notices;",
                    )
                    .map_err(OpenError::Sqlite)?;
            }
            transaction.execute_batch(sql).map_err(OpenError::Sqlite)?;
        }
    }
    Ok(())
}

/// Create the principal head index an authority at `version` lacks; version
/// zero lacks it.
fn create_principal_heads(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<(), OpenError> {
    if version < PRINCIPAL_HEADS_SCHEMA {
        transaction
            .execute_batch(PRINCIPAL_HEADS_SQL)
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

/// Refuse a predecessor whose retained task authority omits its creation
/// context, before the crossing changes anything.
///
/// The current wire requires the field, so such a row could never be read
/// again. Inspection reads the stored JSON directly rather than any decoder.
fn refuse_creation_omission(connection: &Connection) -> Result<(), OpenError> {
    let omission = connection
        .query_row(
            "SELECT 'fact_envelopes', CAST(seq AS TEXT) FROM fact_envelopes \
             WHERE kind = 'task::added' \
             AND json_type(CAST(envelope AS TEXT), '$.fact.creation') IS NULL \
             UNION ALL \
             SELECT 'task_heads', stamp FROM task_heads \
             WHERE json_type(CAST(body AS TEXT), '$.creation') IS NULL \
             LIMIT 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(OpenError::Sqlite)?;
    match omission {
        Some((table, row)) => Err(MigrationError::CreationContextOmitted { table, row }.into()),
        None => Ok(()),
    }
}

/// Write the explicit subscription table a retained residency head omits.
///
/// Heads written before release 4047a2a4f carry no table, so each team's
/// task-allocation lifetime comes from the one life all of its lanes share,
/// with fast mode off. A head that already carries the table must decode as
/// current authority and stays byte-for-byte; anything else refuses before
/// the crossing commits.
fn cross_residency_subscriptions(connection: &Connection) -> Result<(), OpenError> {
    let Some((head_seq, body)) = connection
        .query_row(
            "SELECT head_seq, body FROM residency_head WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(());
    };
    let mut head = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(serde_json::Value::Object(head)) => head,
        Ok(_) => return Err(residency_error("the stored head is not a JSON object")),
        Err(error) => {
            return Err(residency_error(format!(
                "the stored head is not JSON: {error}"
            )));
        }
    };
    if head.contains_key("subscriptions") {
        return wire::decode::<wire::ResidencyHead>(&body)
            .map(drop)
            .map_err(|error| {
                residency_error(format!("the current head does not decode: {error}"))
            });
    }
    let lanes = head.get("lanes").cloned().unwrap_or_default();
    let lanes: Vec<wire::LaneResidency> = serde_json::from_value(lanes)
        .map_err(|error| residency_error(format!("the lane table does not decode: {error}")))?;
    let subscriptions = derived_subscriptions(&lanes)?;
    head.insert(
        "subscriptions".to_owned(),
        serde_json::to_value(subscriptions).map_err(|error| {
            residency_error(format!("the derived table cannot encode: {error}"))
        })?,
    );
    let current: wire::ResidencyHead = serde_json::from_value(serde_json::Value::Object(head))
        .map_err(|error| residency_error(format!("the crossed head does not decode: {error}")))?;
    let bytes = wire::encode(&current)
        .map_err(|error| residency_error(format!("the crossed head cannot encode: {error}")))?;
    let changed = connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1 AND head_seq = ?2",
            params![bytes, head_seq],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(residency_error(
            "the head changed while its exclusive migration held the authority",
        ));
    }
    Ok(())
}

/// One subscription per team, in team order, from the life its lanes share.
fn derived_subscriptions(
    lanes: &[wire::LaneResidency],
) -> Result<Vec<wire::TeamSubscription>, OpenError> {
    let mut lives = std::collections::BTreeMap::new();
    for row in lanes {
        if *lives.entry(row.team).or_insert(row.life.as_str()) != row.life {
            return Err(residency_error(format!(
                "team {} has mixed lane lives and no authoritative task-allocation lifetime",
                row.team
            )));
        }
    }
    lives
        .into_iter()
        .map(|(team, life)| {
            let policy = wire::intent::TeamLife::from_wire(life).ok_or_else(|| {
                residency_error(format!(
                    "team {team} lane life {life:?} is no closed task-allocation lifetime"
                ))
            })?;
            Ok(wire::TeamSubscription {
                team,
                formed_at: None,
                policy,
                fast_mode: false,
            })
        })
        .collect()
}

fn residency_error(detail: impl Into<String>) -> OpenError {
    residual::retained(
        "residency_head",
        "1".to_owned(),
        RESIDENCY_SUBSCRIPTIONS_RELEASE,
        detail.into(),
    )
}

fn cross_retired_gate_filed(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT seq, wire_version, instant, actor, origin, cause, envelope \
                 FROM fact_envelopes WHERE kind = ?1 ORDER BY seq",
            )
            .map_err(OpenError::Sqlite)?;
        let found = statement
            .query_map([RETIRED_GATE_FILED_KIND], |row| {
                Ok(RetiredGateFiledRow {
                    seq: row.get(0)?,
                    wire_version: row.get(1)?,
                    instant: row.get(2)?,
                    actor: row.get(3)?,
                    origin: row.get(4)?,
                    cause: row.get(5)?,
                    envelope: row.get(6)?,
                })
            })
            .map_err(OpenError::Sqlite)?;
        found
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };

    for row in rows {
        let envelope: wire::Envelope<RetiredGateFiled> = serde_json::from_slice(&row.envelope)
            .map_err(|error| {
                retained_fact_error(
                    row.seq,
                    format!("the stored envelope does not decode: {error}"),
                )
            })?;
        let indexed_cause = row.cause.and_then(|cause| u64::try_from(cause).ok());
        let indexed_seq = u64::try_from(row.seq).ok();
        if !envelope.v.is_current()
            || Some(envelope.seq.get()) != indexed_seq
            || i64::from(envelope.v.0) != row.wire_version
            || envelope.at.to_string() != row.instant
            || envelope.actor.to_string() != row.actor
            || envelope.origin.as_str() != row.origin
            || envelope.cause.map(wire::Seq::get) != indexed_cause
        {
            return Err(retained_fact_error(
                row.seq,
                "the stored envelope does not agree with its indexed columns",
            ));
        }
        let fact = serde_json::to_value(envelope.fact).map_err(|error| {
            retained_fact_error(
                row.seq,
                format!("the retired body cannot become inert evidence: {error}"),
            )
        })?;
        let encoded_fact = wire::encode(&wire::Fact::new(wire::body::JournalRetained {
            release: RETIRED_GATE_FILED_RELEASE.to_owned(),
            fact,
        }))
        .map_err(|error| {
            retained_fact_error(
                row.seq,
                format!("the current evidence marker cannot encode: {error}"),
            )
        })?;
        let current_fact: wire::AnyFact = wire::decode(&encoded_fact).map_err(|error| {
            retained_fact_error(
                row.seq,
                format!("the current evidence marker cannot decode: {error}"),
            )
        })?;
        let replacement = wire::Envelope {
            v: envelope.v,
            seq: envelope.seq,
            at: envelope.at,
            actor: envelope.actor,
            origin: envelope.origin,
            cause: envelope.cause,
            fact: current_fact,
        };
        let bytes = wire::encode(&replacement).map_err(|error| {
            retained_fact_error(
                row.seq,
                format!("the current evidence envelope cannot encode: {error}"),
            )
        })?;
        let changed = transaction
            .execute(
                "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 \
                 WHERE seq = ?3 AND kind = ?4",
                params![
                    wire::FactKind::JournalRetained.as_str(),
                    bytes,
                    row.seq,
                    RETIRED_GATE_FILED_KIND,
                ],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(retained_fact_error(
                row.seq,
                "the retained row changed while its exclusive migration held the authority",
            ));
        }
    }
    Ok(())
}

fn retained_fact_error(seq: i64, detail: impl Into<String>) -> OpenError {
    MigrationError::RetiredFact {
        seq,
        kind: RETIRED_GATE_FILED_KIND,
        release: RETIRED_GATE_FILED_RELEASE,
        detail: detail.into(),
    }
    .into()
}

/// Drop retired plan-advance guidance from retained task completions and the
/// receipts that replay their exact retries.
///
/// Unlike the identity-only crossings, this rewrites retained bytes in place.
/// Each `task::done` envelope and each `task::done` receipt response loses only
/// its guidance member and is re-encoded canonically through the current strict
/// DTO, keeping its seq or command id, indexed columns, and every other field.
/// No digest covers an individual envelope or receipt, so nothing downstream
/// disagrees with the new bytes; the advisory text itself is unrecoverable.
/// Any guidance other than the one retired spelling refuses the crossing.
fn cross_retired_task_guidance(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::TaskDone,
        "json_type(CAST(envelope AS TEXT), '$.fact.terminal.guidance') IS NOT NULL",
        RETIRED_TASK_GUIDANCE_RELEASE,
        |_, fact| {
            take_retired_guidance(
                fact.get_mut("terminal")
                    .and_then(serde_json::Value::as_object_mut),
            )
        },
    )?;

    let command = wire::CommandKind::TaskDone;
    residual::rewrite_receipts(
        transaction,
        command,
        Some("json_type(CAST(response AS TEXT), '$.outcome.body.guidance') IS NOT NULL"),
        RETIRED_TASK_GUIDANCE_RELEASE,
        |_, response| {
            let response = residual::edit_reply(command, response, |answer| {
                take_retired_guidance(Some(answer)).map(|()| true)
            })?;
            Ok((None, response))
        },
    )
}

/// Remove the guidance member, refusing anything but the retired spelling.
fn take_retired_guidance(
    object: Option<&mut serde_json::Map<String, serde_json::Value>>,
) -> Result<(), String> {
    match object.and_then(|object| object.remove("guidance")) {
        Some(serde_json::Value::String(guidance)) if guidance == RETIRED_TASK_GUIDANCE => Ok(()),
        Some(other) => Err(format!(
            "guidance {other} is not the retired plan-advance spelling"
        )),
        None => Err("guidance is not a member of the completion it was indexed under".to_owned()),
    }
}

/// Validate a finalized schema identity before applying the current cutoff.
///
/// Version disagreement and forward authority are corruption/compatibility
/// failures distinct from a retired predecessor. They are classified here
/// before [`admit_state`] applies the exact-current requirement.
fn admit_readable_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
    let version = finalized_stored_state(connection, state)?;
    if version > SCHEMA_VERSION {
        return Err(MigrationError::ForwardSchema {
            found: version,
            current: SCHEMA_VERSION,
        }
        .into());
    }

    Ok(version)
}

fn finalized_stored_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
    if state.state == PREPARED {
        return Err(MigrationError::Unfinalized {
            from: state.from_version,
            to: state.to_version,
        }
        .into());
    }
    if state.state != FINALIZED {
        return Err(MigrationError::UnknownState { state: state.state }.into());
    }

    let pragma: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(OpenError::Sqlite)?;
    if pragma != state.version {
        return Err(MigrationError::VersionDisagreement {
            row: state.version,
            pragma,
        }
        .into());
    }
    Ok(state.version)
}

const RETIRED_RELEASES: [&str; 32] = [
    "122d2a842",
    "IDENTIT-1kLGl2S5",
    "IDENTIT-1kLGl2S5",
    "b6acb226d",
    "d0c14dcc4",
    "SESSION-1kLLCyqw",
    "MINING-1kLNTy1C",
    "MINING-1kLNTy1C",
    "87636d7af",
    "1kRNj6X7",
    "257200978",
    "6ad57c713",
    "d811711a3",
    "ENVIRON-dRyzKgHJw",
    "SCHEMAR-dRyzNKsdp",
    "b9a45a6ec",
    "37cc1297f",
    "ALLOCAT-dS8cvNplX",
    "BOUNDS-dSHsMK0kz",
    "eab8365a6",
    "d431530eb",
    "c64c48cac",
    "TASKS-dSPJyjNSH",
    "TASKS-dSPW61MXd",
    "fb96c12a7",
    "966d33bef",
    "916081f8a",
    "02715f9e1",
    "SPEECH-dTQBpHHJq",
    "a4abe071f",
    "55ad00080",
    "a15ed13f4",
];

fn retired_release(version: u32) -> Option<&'static str> {
    let index = usize::try_from(version.checked_sub(1)?).ok()?;
    RETIRED_RELEASES.get(index).copied()
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, OpenError> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            params![table],
            |_| Ok(()),
        )
        .optional()
        .map(|row| row.is_some())
        .map_err(OpenError::Sqlite)
}

fn authority_object_count(connection: &Connection) -> Result<u32, OpenError> {
    connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(OpenError::Sqlite)
}

struct SchemaState {
    version: u32,
    state: String,
    from_version: Option<u32>,
    to_version: Option<u32>,
}

/// A database open, schema admission, or SQLite operation failure.
#[derive(Debug)]
pub enum OpenError {
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    Sqlite(rusqlite::Error),
    Migration(MigrationError),
    /// Schema admission was attempted before disabling trusted schema objects.
    TrustedSchemaEnabled,
    InvalidCursor {
        found: i64,
    },
    /// The on-disk authority would not enter write-ahead-log mode.
    JournalMode {
        found: String,
    },
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { path, source } => {
                write!(
                    formatter,
                    "journal could not open {}: {source}",
                    path.display()
                )
            }
            Self::Sqlite(error) => write!(formatter, "journal SQLite operation failed: {error}"),
            Self::Migration(error) => error.fmt(formatter),
            Self::TrustedSchemaEnabled => {
                formatter.write_str("journal schema admission requires PRAGMA trusted_schema = OFF")
            }
            Self::InvalidCursor { found } => {
                write!(
                    formatter,
                    "journal cursor must be a positive position, found {found}"
                )
            }
            Self::JournalMode { found } => {
                write!(
                    formatter,
                    "journal authority must run in write-ahead-log mode, found {found:?}"
                )
            }
        }
    }
}

impl Error for OpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::Sqlite(error) => Some(error),
            Self::Migration(error) => Some(error),
            Self::InvalidCursor { .. } | Self::JournalMode { .. } | Self::TrustedSchemaEnabled => {
                None
            }
        }
    }
}

impl From<MigrationError> for OpenError {
    fn from(error: MigrationError) -> Self {
        Self::Migration(error)
    }
}

/// A named schema-state refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationError {
    MissingSchema,
    UnversionedAuthority {
        object_count: u32,
    },
    SchemaRowCount {
        count: u32,
    },
    MissingSingleton,
    Unfinalized {
        from: Option<u32>,
        to: Option<u32>,
    },
    UnknownState {
        state: String,
    },
    ForwardSchema {
        found: u32,
        current: u32,
    },
    UnknownVersion {
        found: u32,
    },
    /// Predecessor authority that only a writable open crosses forward.
    PendingCrossing {
        found: u32,
    },
    /// Retained task authority omitting the creation context its schema requires.
    CreationContextOmitted {
        table: String,
        row: String,
    },
    /// Retained data that cannot cross without its last owning release.
    RetiredFact {
        seq: i64,
        kind: &'static str,
        release: &'static str,
        detail: String,
    },
    /// Retained authority row that cannot cross without its last owning release.
    RetainedRow {
        table: &'static str,
        row: String,
        release: &'static str,
        detail: String,
    },
    VersionDisagreement {
        row: u32,
        pragma: u32,
    },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSchema => formatter.write_str(
                "journal has no schema identity; release 1 requires a finalized schema before read-only inspection",
            ),
            Self::UnversionedAuthority { object_count } => write!(
                formatter,
                "journal refuses an unversioned nonempty authority containing {object_count} schema objects; release 1 requires backup and an empty authority"
            ),
            Self::SchemaRowCount { count } => write!(
                formatter,
                "journal schema identity requires exactly one row, found {count}"
            ),
            Self::MissingSingleton => {
                formatter.write_str("journal schema singleton row 1 is missing")
            }
            Self::Unfinalized { from, to } => write!(
                formatter,
                "journal migration remains prepared and unfinalized from {from:?} to {to:?}"
            ),
            Self::UnknownState { state } => write!(
                formatter,
                "journal schema state {state:?} is unknown to release 1"
            ),
            Self::ForwardSchema { found, current } => write!(
                formatter,
                "journal schema {found} is newer than this build's schema {current}; use the release that owns schema {found}"
            ),
            Self::PendingCrossing { found } => write!(
                formatter,
                "journal schema {found} crosses once to schema {SCHEMA_VERSION} at the next \
                 writable open; start the resident with this build before reading it"
            ),
            Self::CreationContextOmitted { table, row } => write!(
                formatter,
                "journal {table} row {row} omits task creation context, which release \
                 {CREATION_CONTEXT_RELEASE} migrated into every task fact and head; restore \
                 authority that release crossed before opening it with this build"
            ),
            Self::UnknownVersion { found } => {
                if let Some(release) = retired_release(*found) {
                    write!(
                        formatter,
                        "journal schema {found} was last migrated by release {release}; \
                         use that release to reach schema {SCHEMA_VERSION} before opening it \
                         with this build"
                    )
                } else {
                    write!(
                        formatter,
                        "journal schema {found} has no migration path to schema \
                         {SCHEMA_VERSION}; use the release that owns schema {found} to reach \
                         schema {SCHEMA_VERSION} first"
                    )
                }
            }
            Self::RetiredFact {
                seq,
                kind,
                release,
                detail,
            } => write!(
                formatter,
                "journal fact {seq} kind {kind} was last owned by release {release} and cannot \
                 cross to schema {SCHEMA_VERSION}: {detail}"
            ),
            Self::RetainedRow {
                table,
                row,
                release,
                detail,
            } => write!(
                formatter,
                "journal {table} row {row} was last shaped by release {release} and cannot \
                 cross to schema {SCHEMA_VERSION}: {detail}"
            ),
            Self::VersionDisagreement { row, pragma } => write!(
                formatter,
                "journal schema row names version {row}, but SQLite user_version names {pragma}"
            ),
        }
    }
}

impl Error for MigrationError {}

const BASELINE_SQL: &str = r#"
CREATE TABLE journal_schema (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL CHECK (version > 0),
    state TEXT NOT NULL CHECK (state IN ('prepared', 'finalized')),
    from_version INTEGER,
    to_version INTEGER,
    plan_digest TEXT,
    CHECK (
        (state = 'finalized' AND from_version IS NULL AND to_version IS NULL AND plan_digest IS NULL)
        OR
        (state = 'prepared' AND to_version IS NOT NULL AND plan_digest IS NOT NULL)
    )
) STRICT;

CREATE TABLE fact_envelopes (
    seq INTEGER PRIMARY KEY CHECK (seq > 0),
    wire_version INTEGER NOT NULL CHECK (wire_version > 0),
    instant TEXT NOT NULL,
    actor TEXT NOT NULL,
    origin TEXT NOT NULL,
    cause INTEGER REFERENCES fact_envelopes(seq) DEFERRABLE INITIALLY DEFERRED,
    kind TEXT NOT NULL,
    envelope BLOB NOT NULL,
    CHECK (cause IS NULL OR cause < seq)
) STRICT;
CREATE INDEX fact_envelopes_origin_seq ON fact_envelopes(origin, seq);
CREATE INDEX fact_envelopes_cause ON fact_envelopes(cause) WHERE cause IS NOT NULL;
CREATE INDEX fact_envelopes_kind_seq ON fact_envelopes(kind, seq);
CREATE INDEX fact_envelopes_origin_kind_seq ON fact_envelopes(origin, kind, seq);

CREATE TABLE thread_index (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    origin TEXT NOT NULL,
    cause INTEGER REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    root INTEGER NOT NULL REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    depth INTEGER NOT NULL CHECK (depth >= 0),
    CHECK ((cause IS NULL AND root = seq AND depth = 0) OR cause IS NOT NULL)
) STRICT;
CREATE INDEX thread_index_origin_seq ON thread_index(origin, seq);
CREATE INDEX thread_index_root_seq ON thread_index(root, seq);

CREATE TABLE command_receipts (
    command_id TEXT PRIMARY KEY,
    actor TEXT NOT NULL,
    command_kind TEXT NOT NULL,
    request BLOB NOT NULL,
    evaluated_witnesses BLOB NOT NULL,
    response BLOB NOT NULL,
    first_seq INTEGER REFERENCES fact_envelopes(seq),
    last_seq INTEGER REFERENCES fact_envelopes(seq),
    snapshot_seq INTEGER REFERENCES fact_envelopes(seq),
    epoch TEXT NOT NULL,
    CHECK (
        (first_seq IS NOT NULL AND last_seq IS NOT NULL AND snapshot_seq IS NULL AND first_seq <= last_seq)
        OR
        (first_seq IS NULL AND last_seq IS NULL AND snapshot_seq IS NOT NULL)
    )
) STRICT;

CREATE TABLE task_heads (
    stamp TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX task_heads_project_stamp
ON task_heads(json_extract(CAST(body AS TEXT), '$.project'), stamp);
CREATE TABLE release_landings (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    commit_id TEXT NOT NULL,
    task TEXT NOT NULL REFERENCES task_heads(stamp) ON DELETE RESTRICT,
    project TEXT NOT NULL,
    title TEXT NOT NULL,
    contributes INTEGER NOT NULL CHECK (contributes IN (0, 1)),
    verdict TEXT CHECK (verdict IN ('clean', 'finding'))
) STRICT;
CREATE UNIQUE INDEX release_landings_contribution
    ON release_landings(commit_id) WHERE contributes = 1;
CREATE INDEX release_landings_commit_seq ON release_landings(commit_id, seq);
CREATE INDEX release_landings_task_seq ON release_landings(task, seq);
CREATE TABLE release_corrections (
    correction TEXT PRIMARY KEY REFERENCES task_heads(stamp) ON DELETE RESTRICT,
    parent TEXT NOT NULL REFERENCES task_heads(stamp) ON DELETE RESTRICT,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    CHECK (correction <> parent)
) STRICT;
CREATE TABLE workflow_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL,
    revised_seq INTEGER REFERENCES fact_envelopes(seq)
) STRICT;
CREATE INDEX task_heads_head_seq ON task_heads(head_seq);
CREATE INDEX workflow_heads_head_seq ON workflow_heads(head_seq);
CREATE INDEX workflow_heads_revised_seq ON workflow_heads(revised_seq)
    WHERE revised_seq IS NOT NULL;
CREATE TABLE task_generations (
    stamp TEXT NOT NULL REFERENCES task_heads(stamp),
    generation INTEGER NOT NULL CHECK (generation > 0),
    seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    baseline INTEGER NOT NULL DEFAULT 0 CHECK (baseline IN (0, 1)),
    PRIMARY KEY (stamp, generation)
) STRICT, WITHOUT ROWID;
CREATE TABLE claim_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE claim_index (
    stamp TEXT PRIMARY KEY REFERENCES claim_heads(stamp),
    actor TEXT NOT NULL,
    lane TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (disposition IN ('active', 'freed', 'lapsed')),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
) STRICT;
CREATE INDEX claim_index_actor_disposition_stamp
    ON claim_index(actor, disposition, stamp, lane, head_seq);
CREATE INDEX claim_index_disposition_actor
    ON claim_index(disposition, actor);
CREATE INDEX claim_index_disposition_lane_stamp
    ON claim_index(disposition, lane, stamp, head_seq);
CREATE TABLE lease_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE witness_heads (
    name TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE trust_heads (
    digest TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE finding_heads (
    identity TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE transcript_source_heads (
    stream TEXT PRIMARY KEY,
    lane TEXT NOT NULL,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX transcript_source_heads_lane_stream
    ON transcript_source_heads(lane, stream, head_seq);
CREATE TABLE steering_heads (
    directive TEXT PRIMARY KEY,
    steer_key TEXT NOT NULL UNIQUE,
    lane TEXT NOT NULL,
    disposition TEXT NOT NULL,
    created_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    minted_micros INTEGER,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL,
    room TEXT
) STRICT;
CREATE INDEX steering_heads_lane_disposition_created
    ON steering_heads(lane, disposition, created_seq);
CREATE INDEX steering_heads_disposition_created
    ON steering_heads(disposition, created_seq);
CREATE INDEX steering_heads_minted_micros
    ON steering_heads(minted_micros) WHERE minted_micros IS NOT NULL;
CREATE INDEX steering_heads_lane_room_created
    ON steering_heads(lane, room, created_seq);
CREATE TABLE steering_misses (
    identity TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE steering_index (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    indexed_seq INTEGER REFERENCES fact_envelopes(seq)
) STRICT;
CREATE TABLE residency_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE cursor_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    seq INTEGER REFERENCES fact_envelopes(seq),
    wake INTEGER REFERENCES fact_envelopes(seq),
    CHECK (wake IS NULL OR (seq IS NOT NULL AND wake <= seq))
) STRICT;

CREATE TABLE taste_fires (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq),
    taste TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    UNIQUE(taste, ordinal)
) STRICT;
CREATE INDEX taste_fires_taste_seq ON taste_fires(taste, seq DESC);
CREATE TABLE taste_judgments (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq),
    taste TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    confirmed INTEGER NOT NULL CHECK (confirmed >= 0),
    cleared INTEGER NOT NULL CHECK (cleared >= 0),
    UNIQUE(taste, ordinal)
) STRICT;
CREATE INDEX taste_judgments_taste_seq ON taste_judgments(taste, seq DESC);
CREATE TABLE lane_seats (
    lane TEXT PRIMARY KEY,
    vendor TEXT NOT NULL,
    seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
) STRICT;

CREATE TRIGGER cursor_head_monotone
BEFORE UPDATE ON cursor_head
WHEN (OLD.seq IS NOT NULL AND (NEW.seq IS NULL OR NEW.seq < OLD.seq))
  OR (OLD.wake IS NOT NULL AND (NEW.wake IS NULL OR NEW.wake < OLD.wake))
BEGIN
    SELECT RAISE(ABORT, 'cursor head cannot move backward');
END;

INSERT INTO cursor_head(singleton, seq, wake) VALUES (1, NULL, NULL);
INSERT INTO steering_index(singleton, indexed_seq) VALUES (1, NULL);
"#;
