//! Retained task creation contexts cross onto the current closed reasons.

use super::*;

/// The pre-context evidence reason, last stamped by the schema-74 OOPS crossing.
const PRE_CONTEXT: &str = "legacy-evidence-missing";
/// The unregistered-stream reason, last written by the context crossings
/// 9fdb23fc1 retired.
const UNREGISTERED: &str = "legacy-stream-unregistered";
/// A spelling no release wrote.
const UNKNOWN: &str = "pre-context-evidence";

fn unavailable(reason: &str) -> String {
    json!({"kind": "unavailable", "reason": reason}).to_string()
}

fn transcript(stream: &str) -> String {
    json!({
        "kind": "transcript",
        "session": "session",
        "boundary": {
            "lane": "01234567-89ab-cdef-0123-456789abcdef",
            "vendor": "omp",
            "stream": stream,
            "source": "fedcba98-7654-3210-fedc-ba9876543210",
            "next": {"member": 1, "offset": 0}
        }
    })
    .to_string()
}

fn lane() -> Actor {
    Actor::Lane(uuid("01234567-89ab-cdef-0123-456789abcdef"))
}

/// A schema-predecessor fixture whose creating fact (seq 1, written by
/// `actor`) and statement head hold the given raw creation contexts.
fn predecessor(actor: &Actor, fact: &str, head: &str) -> Journal {
    let journal = authority_fixture();
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET actor = ?1, envelope = CAST(json_set(CAST(envelope AS \
             TEXT), '$.actor', ?1, '$.fact.creation', json(?2)) AS BLOB) WHERE seq = 1",
            params![actor.to_string(), fact],
        )
        .unwrap();
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), '$.creation', \
             json(?1)) AS BLOB) WHERE stamp = '1kKRRsDP'",
            [head],
        )
        .unwrap();
    restore_predecessor(&journal, SCHEMA_VERSION - 1);
    journal
}

fn rows(journal: &Journal) -> Vec<(i64, String, Vec<u8>)> {
    journal
        .connection
        .prepare(
            "SELECT seq, actor, envelope FROM fact_envelopes \
             UNION ALL SELECT head_seq, stamp, body FROM task_heads ORDER BY 1, 2",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn a_retired_reason_crosses_to_the_reason_its_creating_actor_proves_on_both_copies() {
    let spice = wire::body::TaskCreationUnavailable::SpiceActor;
    let operator = wire::body::TaskCreationUnavailable::OperatorActor;
    for (actor, fact, head, reason) in [
        (
            Actor::Spice,
            unavailable(PRE_CONTEXT),
            unavailable(PRE_CONTEXT),
            spice,
        ),
        (
            Actor::Spice,
            unavailable(spice.as_str()),
            unavailable(PRE_CONTEXT),
            spice,
        ),
        (
            Actor::Operator,
            unavailable(UNKNOWN),
            unavailable(operator.as_str()),
            operator,
        ),
    ] {
        let mut journal = predecessor(&actor, &fact, &head);
        let before = rows(&journal);

        let migration = crate::migrate::admit(&mut journal.connection)
            .unwrap()
            .unwrap();
        assert_eq!(
            (migration.from(), migration.to()),
            (SCHEMA_VERSION - 1, SCHEMA_VERSION)
        );
        let current = wire::body::TaskCreationContext::unavailable(reason);
        let bytes: Vec<u8> = journal
            .connection
            .query_row(
                "SELECT envelope FROM fact_envelopes WHERE seq = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let envelope: Envelope<AnyFact> = wire::decode(&bytes).unwrap();
        assert_eq!(
            wire::encode(&envelope).unwrap(),
            bytes,
            "{fact} / {head}: the fact is canonical"
        );
        assert_eq!(envelope.seq, Seq::new(1).unwrap());
        assert_eq!(envelope.actor, actor);
        let added: wire::body::TaskAdded =
            envelope.fact.read().expect("seq 1 stays the creating fact");
        assert_eq!(added.creation, current, "{fact} / {head}");
        let bytes: Vec<u8> = journal
            .connection
            .query_row(
                "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let statement: TaskHead = wire::decode(&bytes).unwrap();
        assert_eq!(statement.creation, current, "{fact} / {head}");
        assert_eq!(
            rows(&journal).len(),
            before.len(),
            "{fact} / {head}: the crossing rewrites in place"
        );
        journal.verify().expect("the crossed creation replays");
        let crossed = rows(&journal);
        assert_eq!(
            crate::migrate::admit(&mut journal.connection).unwrap(),
            None
        );
        assert_eq!(rows(&journal), crossed);
    }
}

#[test]
fn an_unproved_or_contradictory_creation_context_rolls_back_the_entire_crossing() {
    let spice = unavailable("spice-actor");
    let operator = unavailable("operator-actor");
    for (actor, fact, head, table, row, release) in [
        (
            lane(),
            unavailable(PRE_CONTEXT),
            unavailable(PRE_CONTEXT),
            "fact_envelopes",
            "1",
            "1aa1fcf3f",
        ),
        (
            Actor::Spice,
            unavailable(PRE_CONTEXT),
            operator.clone(),
            "fact_envelopes",
            "1",
            "1aa1fcf3f",
        ),
        (
            Actor::Spice,
            transcript("session"),
            unavailable(UNREGISTERED),
            "task_heads",
            "1kKRRsDP",
            "f55963cab",
        ),
        (
            lane(),
            unavailable(UNKNOWN),
            unavailable(UNKNOWN),
            "fact_envelopes",
            "1",
            "6ad57c713",
        ),
        (
            Actor::Spice,
            transcript("stdout"),
            spice.clone(),
            "fact_envelopes",
            "1",
            "f8be0f9bb",
        ),
        (
            Actor::Spice,
            spice.clone(),
            transcript("stdout"),
            "task_heads",
            "1kKRRsDP",
            "f8be0f9bb",
        ),
    ] {
        let mut journal = predecessor(&actor, &fact, &head);
        assert_refused(
            &mut journal,
            table,
            row,
            release,
            &format!("{fact} / {head}"),
        );
    }

    let mut journal = predecessor(&Actor::Spice, &spice, &unavailable(PRE_CONTEXT));
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.task', '1kKRRsDQ') AS BLOB) WHERE seq = 1",
            [],
        )
        .unwrap();
    assert_refused(
        &mut journal,
        "task_heads",
        "1kKRRsDP",
        "1aa1fcf3f",
        "a retired head with no creating fact",
    );
}

/// The crossing refuses on `table` row `row`, naming `release`, and leaves
/// the predecessor untouched.
fn assert_refused(journal: &mut Journal, table: &str, row: &str, release: &str, case: &str) {
    let before = rows(journal);
    let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
    assert!(
        matches!(
            &error,
            OpenError::Migration(MigrationError::RetainedRow {
                table: refused,
                row: refused_row,
                release: refused_release,
                ..
            }) if *refused == table && refused_row == row && *refused_release == release
        ),
        "{case}: {error}"
    );
    assert_eq!(rows(journal), before, "{case}");
    assert_eq!(
        crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
        SCHEMA_VERSION - 1
    );
}
