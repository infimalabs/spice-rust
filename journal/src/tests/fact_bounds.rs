//! Durable fact-envelope size admission and schema certification.

use super::*;
use crate::{FactCause, JournalWrite};

fn note_envelope_length(note_bytes: usize, position: u64) -> usize {
    let fact = wire::Fact::new(wire::body::TaskNoted {
        task: "1kKRRsDP".parse().expect("fixture task"),
        note: "x".repeat(note_bytes),
    });
    let fact_bytes = wire::encode(&fact).expect("fixture fact encodes");
    let fact = wire::decode::<AnyFact>(&fact_bytes).expect("fixture fact erases");
    wire::encode(&Envelope {
        v: WireVersion::CURRENT,
        seq: seq(position),
        at: instant(),
        actor: Actor::Operator,
        origin: "task:1kKRRsDP".parse().expect("fixture origin"),
        cause: Some(seq(position - 1)),
        fact,
    })
    .expect("fixture envelope encodes")
    .len()
}

fn note_bytes_for_envelope(target: usize, position: u64) -> usize {
    let fixed = note_envelope_length(0, position);
    let note = target
        .checked_sub(fixed)
        .expect("target exceeds fixed wire");
    assert_eq!(note_envelope_length(note, position), target);
    note
}

fn note_write(note_bytes: usize) -> JournalWrite {
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:1kKRRsDP".parse().expect("fixture origin"),
        instant(),
        wire::Producer::CommandDoor,
    );
    write
        .fact(
            wire::body::TaskNoted {
                task: "1kKRRsDP".parse().expect("fixture task"),
                note: "x".repeat(note_bytes),
            },
            FactCause::Existing(seq(2)),
        )
        .expect("fixture note builds");
    write
}

#[test]
fn writes_admit_the_exact_limit_and_refuse_a_larger_batch_before_any_effect() {
    let mut exact = authority_fixture();
    let exact_note = note_bytes_for_envelope(MAX_DURABLE_FACT_ENVELOPE_BYTES, 3);
    let applied = exact
        .apply(note_write(exact_note))
        .expect("exact-limit fact commits");
    assert_eq!(applied.last(), seq(3));
    let stored: i64 = exact
        .connection
        .query_row(
            "SELECT length(envelope) FROM fact_envelopes WHERE seq = 3",
            [],
            |row| row.get(0),
        )
        .expect("exact-limit fact length reads");
    assert_eq!(stored, MAX_DURABLE_FACT_ENVELOPE_BYTES as i64);

    let mut refused = authority_fixture();
    let before = authority_shape(&refused.connection);
    let mut batch = note_write(1);
    batch
        .fact(
            wire::body::TaskNoted {
                task: "1kKRRsDP".parse().expect("fixture task"),
                note: "x".repeat(note_bytes_for_envelope(
                    MAX_DURABLE_FACT_ENVELOPE_BYTES + 1,
                    4,
                )),
            },
            FactCause::Existing(seq(2)),
        )
        .expect("oversized member remains valid wire");
    let error = refused
        .apply(batch)
        .expect_err("limit-plus-one batch is refused");
    assert!(error.to_string().contains("sequence 4"));
    assert!(
        error
            .to_string()
            .contains(&(MAX_DURABLE_FACT_ENVELOPE_BYTES + 1).to_string())
    );
    assert_eq!(authority_shape(&refused.connection), before);
}

fn authority_shape(connection: &Connection) -> (i64, Option<i64>, i64, i64, i64, i64) {
    let facts = connection
        .query_row("SELECT count(*) FROM fact_envelopes", [], |row| row.get(0))
        .expect("fact count reads");
    let cursor = connection
        .query_row(
            "SELECT seq FROM cursor_head WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("cursor reads");
    let heads = connection
        .query_row(
            "SELECT (SELECT count(*) FROM task_heads) + (SELECT count(*) FROM workflow_heads)",
            [],
            |row| row.get(0),
        )
        .expect("head count reads");
    let receipts = connection
        .query_row("SELECT count(*) FROM command_receipts", [], |row| {
            row.get(0)
        })
        .expect("receipt count reads");
    let threads = connection
        .query_row("SELECT count(*) FROM thread_index", [], |row| row.get(0))
        .expect("thread count reads");
    let steering = connection
        .query_row(
            "SELECT indexed_seq FROM steering_index WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("steering cursor reads");
    (facts, cursor, heads, receipts, threads, steering)
}

fn downgrade_to_twenty(connection: &Connection) {
    connection
        .execute_batch(
            "DROP TRIGGER fact_envelopes_envelope_size_insert;
             DROP TRIGGER fact_envelopes_envelope_size_update;
             UPDATE journal_schema SET version = 20 WHERE singleton = 1;
             PRAGMA user_version = 20;",
        )
        .expect("restore schema twenty");
}

fn append_copies(connection: &Connection, count: u64) {
    if count == 0 {
        return;
    }
    let first: i64 = connection
        .query_row("SELECT max(seq) + 1 FROM fact_envelopes", [], |row| {
            row.get(0)
        })
        .expect("copy range starts");
    let last = first + i64::try_from(count).expect("copy count fits SQLite") - 1;
    connection
        .execute_batch(&format!(
            "WITH RECURSIVE facts(seq) AS (
                 SELECT {first} UNION ALL SELECT seq + 1 FROM facts WHERE seq < {last}
             )
             INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope)
             SELECT facts.seq, source.wire_version, source.instant, source.actor,
                    source.origin, NULL, source.kind, source.envelope
             FROM facts CROSS JOIN fact_envelopes AS source WHERE source.seq = 1;"
        ))
        .expect("append schema-twenty copies");
}

fn version_and_triggers(connection: &Connection) -> (u32, u32, i64) {
    let row = connection
        .query_row(
            "SELECT version FROM journal_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("schema row reads");
    let pragma = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("schema pragma reads");
    let triggers = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'trigger' AND name LIKE 'fact_envelopes_envelope_size_%'",
            [],
            |row| row.get(0),
        )
        .expect("bound triggers read");
    (row, pragma, triggers)
}

#[test]
fn schema_twenty_one_refreshes_its_one_mib_triggers_without_rewriting_facts() {
    let scratch = Scratch::new();
    let path = scratch
        .path()
        .join("schema-twenty-one-trigger-refresh.sqlite3");
    let mut journal = authority_file_fixture(&path);
    let retained = journal
        .apply(note_write(note_bytes_for_envelope(
            MAX_RETAINED_FACT_PAGE_BYTES + 1,
            3,
        )))
        .expect("current authority admits a fact above the page target");
    assert_eq!(retained.last(), seq(3));
    let legacy_limit = MAX_RETAINED_FACT_PAGE_BYTES;
    journal
        .connection
        .execute_batch(&format!(
            "DROP TRIGGER fact_envelopes_envelope_size_insert;
             DROP TRIGGER fact_envelopes_envelope_size_update;
             CREATE TRIGGER fact_envelopes_envelope_size_insert
             BEFORE INSERT ON fact_envelopes WHEN length(NEW.envelope) > {legacy_limit}
             BEGIN SELECT RAISE(ABORT, 'fact envelope exceeds {legacy_limit} bytes'); END;
             CREATE TRIGGER fact_envelopes_envelope_size_update
             BEFORE UPDATE OF envelope ON fact_envelopes WHEN length(NEW.envelope) > {legacy_limit}
             BEGIN SELECT RAISE(ABORT, 'fact envelope exceeds {legacy_limit} bytes'); END;
             UPDATE journal_schema SET version = 21 WHERE singleton = 1;
             PRAGMA user_version = 21;"
        ))
        .expect("restore schema twenty-one trigger contract");
    drop(journal);

    let migrated = Journal::open(&path).expect("schema twenty-one refreshes to current");
    assert_eq!(
        version_and_triggers(&migrated.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION, 2)
    );
    let retained_length: i64 = migrated
        .connection
        .query_row(
            "SELECT length(envelope) FROM fact_envelopes WHERE seq = 3",
            [],
            |row| row.get(0),
        )
        .expect("retained fact length reads");
    assert!(retained_length > MAX_RETAINED_FACT_PAGE_BYTES as i64);
    for trigger in [
        crate::migrate::FACT_ENVELOPE_INSERT_TRIGGER,
        crate::migrate::FACT_ENVELOPE_UPDATE_TRIGGER,
    ] {
        let sql: String = migrated
            .connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |row| row.get(0),
            )
            .expect("refreshed trigger reads");
        assert!(sql.contains(&MAX_DURABLE_FACT_ENVELOPE_BYTES.to_string()));
    }
}

#[test]
fn schema_twenty_row_preflight_is_bounded_and_offline_retry_is_restart_safe() {
    let scratch = Scratch::new();
    let exact_path = scratch.path().join("fact-bound-rows-exact.sqlite3");
    let exact = authority_file_fixture(&exact_path);
    downgrade_to_twenty(&exact.connection);
    append_copies(
        &exact.connection,
        crate::migrate::FACT_BOUND_VALIDATION_ROW_LIMIT - 2,
    );
    drop(exact);
    let exact = Journal::open(&exact_path).expect("exact startup row boundary migrates");
    assert_eq!(
        version_and_triggers(&exact.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION, 2)
    );
    drop(exact);

    let path = scratch.path().join("fact-bound-rows.sqlite3");
    let journal = authority_file_fixture(&path);
    downgrade_to_twenty(&journal.connection);
    append_copies(
        &journal.connection,
        crate::migrate::FACT_BOUND_VALIDATION_ROW_LIMIT - 1,
    );
    drop(journal);

    let error = Journal::open(&path).expect_err("startup row preflight refuses");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::FactBoundValidationRowsRequired {
            limit: crate::migrate::FACT_BOUND_VALIDATION_ROW_LIMIT,
            observed,
        }) if observed == crate::migrate::FACT_BOUND_VALIDATION_ROW_LIMIT + 1
    ));
    assert!(error.to_string().contains("`spice journal reindex`"));
    let connection = Connection::open(&path).expect("schema twenty reopens");
    assert_eq!(version_and_triggers(&connection), (20, 20, 0));
    drop(connection);

    let mut connection = Connection::open(&path).expect("offline transaction opens");
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
        .expect("offline transaction starts");
    crate::migrate::advance_for_reindex(&transaction).expect("offline validation advances");
    assert_eq!(
        version_and_triggers(&transaction),
        (SCHEMA_VERSION, SCHEMA_VERSION, 2)
    );
    drop(transaction);
    drop(connection);
    let connection = Connection::open(&path).expect("rolled-back schema reopens");
    assert_eq!(version_and_triggers(&connection), (20, 20, 0));
    drop(connection);

    let migrated = Journal::open_for_reindex(&path).expect("offline retry commits");
    assert_eq!(
        version_and_triggers(&migrated.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION, 2)
    );
}

#[test]
fn schema_twenty_byte_preflight_admits_exact_and_refuses_one_more() {
    let scratch = Scratch::new();
    for (name, total, succeeds) in [
        (
            "exact",
            crate::migrate::FACT_BOUND_VALIDATION_BYTE_LIMIT,
            true,
        ),
        (
            "plus-one",
            crate::migrate::FACT_BOUND_VALIDATION_BYTE_LIMIT + 1,
            false,
        ),
    ] {
        let path = scratch
            .path()
            .join(format!("fact-bound-bytes-{name}.sqlite3"));
        let journal = authority_file_fixture(&path);
        downgrade_to_twenty(&journal.connection);
        let full = total / MAX_RETAINED_FACT_PAGE_BYTES as u64;
        let remainder = total % MAX_RETAINED_FACT_PAGE_BYTES as u64;
        let rows = full + u64::from(remainder != 0);
        append_copies(&journal.connection, rows - 2);
        journal
            .connection
            .execute(
                "UPDATE fact_envelopes SET envelope = zeroblob(?1) WHERE seq <= ?2",
                params![MAX_RETAINED_FACT_PAGE_BYTES as i64, full as i64],
            )
            .expect("fill complete metadata-only rows");
        if remainder != 0 {
            journal
                .connection
                .execute(
                    "UPDATE fact_envelopes SET envelope = zeroblob(?1) WHERE seq = ?2",
                    params![remainder as i64, rows as i64],
                )
                .expect("fill remainder metadata-only row");
        }
        drop(journal);
        if succeeds {
            Journal::open(&path).expect("exact cumulative byte boundary migrates");
        } else {
            let error = Journal::open(&path).expect_err("one cumulative byte more refuses");
            assert!(matches!(
                error,
                OpenError::Migration(MigrationError::FactBoundValidationBytesRequired {
                    limit: crate::migrate::FACT_BOUND_VALIDATION_BYTE_LIMIT,
                    observed,
                }) if observed == total
            ));
        }
    }
}

#[test]
fn migration_rejects_an_oversized_row_and_finalized_triggers_defend_insert_and_update() {
    let scratch = Scratch::new();
    let path = scratch.path().join("oversized-schema-twenty.sqlite3");
    let journal = authority_file_fixture(&path);
    downgrade_to_twenty(&journal.connection);
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = zeroblob(?1) WHERE seq = 2",
            [MAX_DURABLE_FACT_ENVELOPE_BYTES as i64 + 1],
        )
        .expect("plant oversized predecessor row");
    drop(journal);
    let error =
        Journal::open_for_reindex(&path).expect_err("offline validation rejects corruption");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::OversizedFactEnvelope {
            seq: 2,
            observed,
            limit,
        }) if observed == MAX_DURABLE_FACT_ENVELOPE_BYTES as u64 + 1
            && limit == MAX_DURABLE_FACT_ENVELOPE_BYTES as u64
    ));
    let connection = Connection::open(&path).expect("refused predecessor reopens");
    assert_eq!(version_and_triggers(&connection), (20, 20, 0));

    let current = authority_fixture();
    let oversized = MAX_DURABLE_FACT_ENVELOPE_BYTES + 1;
    for sql in [
        format!("UPDATE fact_envelopes SET envelope = zeroblob({oversized}) WHERE seq = 1"),
        format!(
            "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope)
             SELECT 3, wire_version, instant, actor, origin, NULL, kind, zeroblob({oversized})
             FROM fact_envelopes WHERE seq = 1"
        ),
    ] {
        let error = current
            .connection
            .execute(&sql, [])
            .expect_err("direct SQL cannot cross the durable size maximum");
        assert!(
            error
                .to_string()
                .contains(&format!("fact envelope exceeds {MAX_DURABLE_FACT_ENVELOPE_BYTES} bytes"))
        );
    }

    current
        .connection
        .execute("DROP TRIGGER fact_envelopes_envelope_size_update", [])
        .expect("remove one enforcement object");
    current
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = zeroblob(?1) WHERE seq = 1",
            [MAX_DURABLE_FACT_ENVELOPE_BYTES as i64 + 1],
        )
        .expect("plant oversized current-schema corruption beneath removed update trigger");
    let violations = refused_violations(&current);
    assert!(violations.iter().any(|violation| {
        violation.class == ViolationClass::Schema
            && violation.subject == crate::migrate::FACT_ENVELOPE_UPDATE_TRIGGER
            && violation.detail.contains("missing")
    }));
    assert!(violations.iter().any(|violation| {
        violation.class == ViolationClass::Envelope
            && violation.subject == "1"
            && violation
                .detail
                .contains(&(MAX_DURABLE_FACT_ENVELOPE_BYTES + 1).to_string())
    }));
}
