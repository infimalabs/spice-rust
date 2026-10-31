//! Retained priority and taste-review standing cross into typed statement
//! fields, each taste hold carrying the evidence its audit notes recorded.

use super::*;

fn predecessor(head_tags: serde_json::Value, fact_tags: serde_json::Value) -> Journal {
    let journal = authority_fixture();
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), '$.tags', json(?1)) AS BLOB)",
            [head_tags.to_string()],
        )
        .unwrap();
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT),
             '$.fact.tags', json(?1)) AS BLOB) WHERE kind = 'task::added'",
            [fact_tags.to_string()],
        )
        .unwrap();
    restore_predecessor(&journal, 66);
    journal
}

/// A schema-73 authority whose head and creation fact spell their holds as
/// plans alone.
fn plan_only_predecessor(head: &str, fact: &str) -> Journal {
    let journal = authority_fixture();
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), '$.taste', ?1) AS BLOB)",
            [head],
        )
        .unwrap();
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT),
             '$.fact.taste', ?1) AS BLOB) WHERE kind = 'task::added'",
            [fact],
        )
        .unwrap();
    restore_predecessor(&journal, 73);
    journal
}

/// One match as a retired audit recorded it, with its current typed form.
fn found(field: wire::StatementField, line: usize, taste: &str, trigger: &str) -> wire::TasteMatch {
    wire::TasteMatch {
        field,
        line,
        taste: taste.to_owned(),
        finding: Some(wire::TasteFinding {
            trigger: trigger.to_owned(),
            column: 7,
            message: "Keep \"one\" path; drop [the rest]\u{1b}".to_owned(),
        }),
    }
}

/// One audit note: its position, its cause, and the matches its read found.
type Audit<'a> = (u64, u64, &'a [wire::TasteMatch]);

/// Append the audit note a retired policy read caused, spelled as its writer
/// spelled it.
fn audit(journal: &Journal, seq: u64, cause: u64, matches: &[wire::TasteMatch]) {
    let outcome = if matches.is_empty() {
        "clear"
    } else {
        "required"
    };
    let matches = matches
        .iter()
        .map(|found| {
            let finding = found.finding.as_ref().unwrap();
            format!(
                "field={:?} taste={:?} trigger={:?} message={:?} line={} column={}",
                found.field.as_str(),
                found.taste,
                finding.trigger,
                finding.message,
                found.line,
                finding.column
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let mut envelope = retired_note(
        seq,
        "1kKRRsDP",
        &format!("taste-review audit policy=abc123 outcome={outcome} matches=[{matches}]"),
    );
    envelope.actor = Actor::Spice;
    envelope.cause = Some(super::seq(cause));
    plant_retired(journal, &envelope);
}

fn statements(journal: &Journal) -> (TaskHead, wire::body::TaskAdded) {
    let body: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE kind = 'task::added'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let envelope: Envelope<AnyFact> = wire::decode(&bytes).unwrap();
    (
        wire::decode(&body).unwrap(),
        envelope.fact.read::<wire::body::TaskAdded>().unwrap(),
    )
}

fn rows(journal: &Journal) -> Vec<Vec<u8>> {
    journal
        .connection
        .prepare(
            "SELECT envelope AS bytes FROM fact_envelopes WHERE kind = 'task::added'
             UNION ALL SELECT body FROM task_heads ORDER BY bytes",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn assert_no_statement_tags(journal: &Journal) {
    let heads: i64 = journal
        .connection
        .query_row(
            "SELECT count(*) FROM task_heads
             WHERE json_type(CAST(body AS TEXT), '$.tags') IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let facts: i64 = journal
        .connection
        .query_row(
            "SELECT count(*) FROM fact_envelopes WHERE kind = 'task::added'
             AND json_type(CAST(envelope AS TEXT), '$.fact.tags') IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((heads, facts), (0, 0));
}

#[test]
fn writable_crossing_lifts_standing_tags_into_typed_fields_once() {
    let mut journal = predecessor(
        json!([
            "taste-review:required",
            "alpha",
            "priority:high",
            "taste-review:plan-prepended",
            "omega"
        ]),
        json!(["alpha", "taste-review:required", "omega"]),
    );
    let opened = found(wire::StatementField::Title, 1, "scope", "while");
    audit(&journal, 3, 1, std::slice::from_ref(&opened));
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (66, SCHEMA_VERSION));

    let (head, added) = statements(&journal);
    assert_no_statement_tags(&journal);
    assert_eq!(head.priority, Some(wire::TaskPriority::High));
    assert_eq!(
        head.taste.and_then(|hold| hold.plan),
        Some(wire::TastePlan::Prepended)
    );
    assert_eq!(added.priority, None);
    assert_eq!(
        added.taste,
        Some(wire::TasteHold {
            plan: Some(wire::TastePlan::Authored),
            matches: vec![opened],
            rechecks: 0,
            opened: None,
        })
    );

    let crossed = rows(&journal);
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(rows(&journal), crossed);
}

#[test]
fn writable_crossing_lifts_audit_evidence_into_plan_only_holds_once() {
    let mut journal = plan_only_predecessor("prepended", "authored");
    let opened = found(
        wire::StatementField::Acceptance,
        2,
        "compatibility",
        "legacy",
    );
    let earlier = found(wire::StatementField::Title, 1, "scope", "while");
    let latest = [
        found(wire::StatementField::Title, 1, "scope", "while"),
        found(wire::StatementField::Description, 3, "fallback", "or else"),
    ];
    audit(&journal, 3, 1, std::slice::from_ref(&opened));
    audit(&journal, 4, 2, &[]);
    audit(&journal, 5, 2, std::slice::from_ref(&earlier));
    audit(&journal, 6, 2, &latest);
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (73, SCHEMA_VERSION));

    let (head, added) = statements(&journal);
    assert_eq!(
        head.taste,
        Some(wire::TasteHold {
            plan: Some(wire::TastePlan::Prepended),
            matches: latest.to_vec(),
            rechecks: 1,
            opened: None,
        })
    );
    assert_eq!(
        added.taste,
        Some(wire::TasteHold {
            plan: Some(wire::TastePlan::Authored),
            matches: vec![opened],
            rechecks: 0,
            opened: None,
        })
    );

    let crossed = rows(&journal);
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(rows(&journal), crossed);
}

#[test]
fn holds_without_their_audit_evidence_roll_back_the_entire_crossing() {
    let held = found(wire::StatementField::Title, 1, "scope", "while");
    let cases: [(&str, &[Audit<'_>]); 3] = [
        ("no audit", &[]),
        (
            "latest read clear",
            &[(3, 1, std::slice::from_ref(&held)), (4, 2, &[])],
        ),
        (
            "creation caused two",
            &[
                (3, 1, std::slice::from_ref(&held)),
                (4, 1, std::slice::from_ref(&held)),
            ],
        ),
    ];
    for (case, audits) in cases {
        let mut journal = plan_only_predecessor("authored", "authored");
        for (seq, cause, matches) in audits {
            audit(&journal, *seq, *cause, matches);
        }
        let before = rows(&journal);
        let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
        assert!(matches!(error, OpenError::Migration(_)), "{case}: {error}");
        assert_eq!(rows(&journal), before, "{case}");
        assert_eq!(
            crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
            73,
            "{case}"
        );
    }
}

#[test]
fn standing_tags_without_one_current_meaning_roll_back_the_entire_crossing() {
    for tags in [
        json!(["priority:urgent"]),
        json!(["priority:high", "priority:low"]),
        json!(["taste-review:plan-prepended"]),
    ] {
        let mut journal = predecessor(tags.clone(), json!([]));
        let before = rows(&journal);
        let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
        assert!(matches!(error, OpenError::Migration(_)), "{tags}: {error}");
        assert_eq!(rows(&journal), before, "{tags}");
        assert_eq!(
            crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
            66,
            "{tags}"
        );
    }
}
